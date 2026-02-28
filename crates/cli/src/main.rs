use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde::Deserialize;
use tracing::{error, info};

// ── TOML config file ────────────────────────────────────────────────

/// Optional TOML configuration. All fields are optional — CLI args and
/// env vars take priority over values loaded from the file.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Config {
    db_path: Option<String>,
    chain_id: Option<u64>,
    log_level: Option<String>,
    listen: Option<String>,
    portal: Option<String>,
    dataset: Option<String>,
    rpc_url: Option<String>,
}

fn load_config(path: &std::path::Path) -> Result<Config> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read config file: {}", path.display()))?;
    toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file: {}", path.display()))
}

// ── CLI definition ──────────────────────────────────────────────────

/// EVM State-as-a-Service — unified CLI.
#[derive(Parser, Debug)]
#[command(name = "evm-state", version, about)]
struct Cli {
    /// Path to the state database.
    #[arg(long, env = "EVM_STATE_DB")]
    db: Option<String>,

    /// Chain ID (1 = Ethereum, 137 = Polygon).
    #[arg(long, env = "EVM_STATE_CHAIN_ID")]
    chain_id: Option<u64>,

    /// Path to a TOML configuration file.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Log level filter (e.g. info, debug, warn).
    #[arg(long, env = "EVM_STATE_LOG")]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Start the HTTP API server.
    Serve {
        /// Listen address (host:port).
        #[arg(long, env = "EVM_STATE_LISTEN")]
        listen: Option<String>,
    },

    /// Run the block replay pipeline from the SQD Network.
    Replay {
        /// SQD Network portal URL.
        #[arg(long, env = "EVM_STATE_PORTAL")]
        portal: Option<String>,

        /// SQD dataset name (auto-detected from chain-id if omitted).
        #[arg(long, env = "EVM_STATE_DATASET")]
        dataset: Option<String>,

        /// Start replay from this block (default: resume from head).
        #[arg(long)]
        from: Option<u64>,

        /// Stop replay at this block (default: open-ended).
        #[arg(long)]
        to: Option<u64>,
    },

    /// Import a JSON Lines snapshot into the state database.
    Import {
        /// Path to .jsonl or .jsonl.zst snapshot file.
        file: PathBuf,
    },

    /// Export the state database to a JSON Lines snapshot.
    Export {
        /// Output path (.jsonl or .jsonl.zst for compressed).
        file: PathBuf,
    },

    /// Validate local state against a remote Ethereum RPC.
    Validate {
        /// Ethereum JSON-RPC URL.
        #[arg(long, env = "EVM_STATE_RPC")]
        rpc_url: Option<String>,

        /// Number of random accounts/slots to sample and compare.
        #[arg(long, default_value = "100")]
        samples: usize,
    },
}

// ── Resolved configuration ──────────────────────────────────────────

/// Fully resolved configuration after merging CLI args, env vars, and
/// config file values. Each field has a concrete value (with defaults
/// applied).
struct Resolved {
    db_path: String,
    chain_id: u64,
    log_level: String,
}

impl Resolved {
    fn from_cli_and_config(cli: &Cli, config: &Config) -> Self {
        Self {
            db_path: cli
                .db
                .clone()
                .or_else(|| config.db_path.clone())
                .unwrap_or_else(|| "./state.mdbx".to_string()),
            chain_id: cli
                .chain_id
                .or(config.chain_id)
                .unwrap_or(137),
            log_level: cli
                .log_level
                .clone()
                .or_else(|| config.log_level.clone())
                .unwrap_or_else(|| "info".to_string()),
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn dataset_for_chain(chain_id: u64) -> Result<&'static str> {
    match chain_id {
        1 => Ok(evm_state_sqd_fetcher::ETHEREUM_DATASET),
        137 => Ok(evm_state_sqd_fetcher::POLYGON_DATASET),
        _ => bail!("no default SQD dataset for chain ID {chain_id}; use --dataset"),
    }
}

// ── Subcommand handlers ─────────────────────────────────────────────

async fn run_serve(resolved: &Resolved, config: &Config, listen: Option<String>) -> Result<()> {
    let listen_addr = listen
        .or_else(|| config.listen.clone())
        .unwrap_or_else(|| "0.0.0.0:3000".to_string());

    let chain_spec = evm_state_chain_spec::from_chain_id(resolved.chain_id)
        .with_context(|| format!("unsupported chain ID: {}", resolved.chain_id))?;

    let db = evm_state_db::StateDb::open(&resolved.db_path)
        .with_context(|| format!("failed to open database at {}", resolved.db_path))?;

    let state = Arc::new(evm_state_api::AppState { db, chain_spec });
    let router = evm_state_api::build_router(state);

    let listener = tokio::net::TcpListener::bind(&listen_addr)
        .await
        .with_context(|| format!("failed to bind to {listen_addr}"))?;

    info!(listen = %listen_addr, db = %resolved.db_path, chain_id = resolved.chain_id, "starting API server");
    axum::serve(listener, router).await?;
    Ok(())
}

async fn run_replay(
    resolved: &Resolved,
    config: &Config,
    portal: Option<String>,
    dataset: Option<String>,
    from: Option<u64>,
    to: Option<u64>,
) -> Result<()> {
    let portal_url = portal
        .or_else(|| config.portal.clone())
        .unwrap_or_else(|| evm_state_sqd_fetcher::DEFAULT_PORTAL.to_string());

    let dataset_name = dataset
        .or_else(|| config.dataset.clone())
        .map(Ok)
        .unwrap_or_else(|| dataset_for_chain(resolved.chain_id).map(|s| s.to_string()))?;

    let chain_spec = evm_state_chain_spec::from_chain_id(resolved.chain_id)
        .with_context(|| format!("unsupported chain ID: {}", resolved.chain_id))?;

    let db = evm_state_db::StateDb::open(&resolved.db_path)
        .with_context(|| format!("failed to open database at {}", resolved.db_path))?;

    let head = db.get_head_block()?;
    let start_block = from.or(head.map(|h| h + 1)).unwrap_or(0);

    info!(
        portal = %portal_url,
        dataset = %dataset_name,
        from = start_block,
        to = to.map(|t| t.to_string()).unwrap_or_else(|| "∞".into()),
        "starting replay pipeline"
    );

    let fetcher = evm_state_sqd_fetcher::SqdFetcher::new(&portal_url, &dataset_name);
    let blocks = fetcher.stream_blocks(start_block, to);

    let stats =
        evm_state_replayer::pipeline::run_pipeline(&db, &chain_spec, blocks, |progress| {
            if progress.blocks_processed % 100 == 0 || progress.blocks_processed == 1 {
                info!(
                    block = progress.block_number,
                    txs = progress.tx_count,
                    processed = progress.blocks_processed,
                    bps = format!("{:.1}", progress.blocks_per_sec()),
                    "replayed"
                );
            }
            true
        })
        .await?;

    info!(
        blocks = stats.blocks_processed,
        first = stats.first_block.unwrap_or(0),
        last = stats.last_block.unwrap_or(0),
        elapsed = ?stats.elapsed,
        bps = format!("{:.1}", stats.blocks_per_sec()),
        "replay complete"
    );

    Ok(())
}

fn run_import(resolved: &Resolved, file: &std::path::Path) -> Result<()> {
    let db = evm_state_db::StateDb::open(&resolved.db_path)
        .with_context(|| format!("failed to open database at {}", resolved.db_path))?;

    info!(file = %file.display(), db = %resolved.db_path, "importing snapshot");

    let stats = evm_state_snapshot::import_snapshot(&db, file, |progress| {
        if progress.entries_imported % 100_000 == 0 && progress.entries_imported > 0 {
            info!(
                entries = progress.entries_imported,
                bytes = progress.bytes_read,
                "import progress"
            );
        }
    })?;

    info!(
        accounts = stats.accounts,
        storage_slots = stats.storage_slots,
        code_entries = stats.code_entries,
        head_block = stats.head_block.unwrap_or(0),
        elapsed = ?stats.elapsed,
        "import complete"
    );

    Ok(())
}

fn run_export(resolved: &Resolved, file: &std::path::Path) -> Result<()> {
    let db = evm_state_db::StateDb::open(&resolved.db_path)
        .with_context(|| format!("failed to open database at {}", resolved.db_path))?;

    info!(file = %file.display(), db = %resolved.db_path, "exporting snapshot");

    let stats = evm_state_snapshot::export_snapshot(&db, file, |progress| {
        if progress.entries_written % 100_000 == 0 && progress.entries_written > 0 {
            info!(
                entries = progress.entries_written,
                "export progress"
            );
        }
    })?;

    info!(
        accounts = stats.accounts,
        storage_slots = stats.storage_slots,
        code_entries = stats.code_entries,
        head_block = stats.head_block.unwrap_or(0),
        elapsed = ?stats.elapsed,
        "export complete"
    );

    Ok(())
}

fn run_validate(
    resolved: &Resolved,
    config: &Config,
    rpc_url: Option<String>,
    samples: usize,
) -> Result<()> {
    let rpc_url = rpc_url
        .or_else(|| config.rpc_url.clone())
        .context("--rpc-url is required for validation")?;

    let db = evm_state_db::StateDb::open(&resolved.db_path)
        .with_context(|| format!("failed to open database at {}", resolved.db_path))?;

    let head = db
        .get_head_block()?
        .context("database has no head block; import or replay first")?;

    info!(rpc = %rpc_url, block = head, samples = samples, "starting validation");

    let rpc = evm_state_validation::EthRpcClient::new(&rpc_url);
    let report = evm_state_validation::validate_random(&db, &rpc, samples, head)?;

    info!(
        total = report.total_checks(),
        mismatches = report.mismatches(),
        valid = report.is_valid(),
        "validation complete"
    );

    if !report.is_valid() {
        for check in &report.account_checks {
            if !check.matches {
                error!(
                    address = %check.address,
                    field = check.field,
                    local = %check.local,
                    remote = %check.remote,
                    "account mismatch"
                );
            }
        }
        for check in &report.storage_checks {
            if !check.matches {
                error!(
                    address = %check.address,
                    slot = %check.slot,
                    local = %check.local,
                    remote = %check.remote,
                    "storage mismatch"
                );
            }
        }
        bail!(
            "validation failed: {} mismatches out of {} checks",
            report.mismatches(),
            report.total_checks()
        );
    }

    Ok(())
}

// ── Entry point ─────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = match &cli.config {
        Some(path) => load_config(path)?,
        None => Config::default(),
    };

    let resolved = Resolved::from_cli_and_config(&cli, &config);

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(&resolved.log_level)
                .with_context(|| format!("invalid log level: {}", resolved.log_level))?,
        )
        .init();

    match cli.command {
        Command::Serve { listen } => run_serve(&resolved, &config, listen).await,
        Command::Replay {
            portal,
            dataset,
            from,
            to,
        } => run_replay(&resolved, &config, portal, dataset, from, to).await,
        Command::Import { file } => run_import(&resolved, &file),
        Command::Export { file } => run_export(&resolved, &file),
        Command::Validate { rpc_url, samples } => run_validate(&resolved, &config, rpc_url, samples),
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ── CLI argument parsing ────────────────────────────────────────

    #[test]
    fn parse_serve_defaults() {
        let cli = Cli::try_parse_from(["evm-state", "serve"]).unwrap();
        assert!(cli.db.is_none());
        assert!(cli.chain_id.is_none());
        assert!(cli.config.is_none());
        assert!(cli.log_level.is_none());
        match cli.command {
            Command::Serve { listen } => assert!(listen.is_none()),
            _ => panic!("expected Serve"),
        }
    }

    #[test]
    fn parse_serve_with_options() {
        let cli = Cli::try_parse_from([
            "evm-state",
            "--db",
            "/tmp/mydb",
            "--chain-id",
            "1",
            "--log-level",
            "debug",
            "serve",
            "--listen",
            "127.0.0.1:8080",
        ])
        .unwrap();

        assert_eq!(cli.db.as_deref(), Some("/tmp/mydb"));
        assert_eq!(cli.chain_id, Some(1));
        assert_eq!(cli.log_level.as_deref(), Some("debug"));
        match cli.command {
            Command::Serve { listen } => assert_eq!(listen.as_deref(), Some("127.0.0.1:8080")),
            _ => panic!("expected Serve"),
        }
    }

    #[test]
    fn parse_replay_defaults() {
        let cli = Cli::try_parse_from(["evm-state", "replay"]).unwrap();
        match cli.command {
            Command::Replay {
                portal,
                dataset,
                from,
                to,
            } => {
                assert!(portal.is_none());
                assert!(dataset.is_none());
                assert!(from.is_none());
                assert!(to.is_none());
            }
            _ => panic!("expected Replay"),
        }
    }

    #[test]
    fn parse_replay_with_options() {
        let cli = Cli::try_parse_from([
            "evm-state",
            "replay",
            "--portal",
            "https://custom.sqd.dev",
            "--dataset",
            "custom-dataset",
            "--from",
            "1000",
            "--to",
            "2000",
        ])
        .unwrap();

        match cli.command {
            Command::Replay {
                portal,
                dataset,
                from,
                to,
            } => {
                assert_eq!(portal.as_deref(), Some("https://custom.sqd.dev"));
                assert_eq!(dataset.as_deref(), Some("custom-dataset"));
                assert_eq!(from, Some(1000));
                assert_eq!(to, Some(2000));
            }
            _ => panic!("expected Replay"),
        }
    }

    #[test]
    fn parse_import() {
        let cli = Cli::try_parse_from(["evm-state", "import", "/tmp/snapshot.jsonl"]).unwrap();
        match cli.command {
            Command::Import { file } => assert_eq!(file, PathBuf::from("/tmp/snapshot.jsonl")),
            _ => panic!("expected Import"),
        }
    }

    #[test]
    fn parse_import_missing_file_fails() {
        assert!(Cli::try_parse_from(["evm-state", "import"]).is_err());
    }

    #[test]
    fn parse_export() {
        let cli = Cli::try_parse_from(["evm-state", "export", "/tmp/snapshot.jsonl.zst"]).unwrap();
        match cli.command {
            Command::Export { file } => assert_eq!(file, PathBuf::from("/tmp/snapshot.jsonl.zst")),
            _ => panic!("expected Export"),
        }
    }

    #[test]
    fn parse_export_missing_file_fails() {
        assert!(Cli::try_parse_from(["evm-state", "export"]).is_err());
    }

    #[test]
    fn parse_validate_defaults() {
        let cli = Cli::try_parse_from(["evm-state", "validate"]).unwrap();
        match cli.command {
            Command::Validate { rpc_url, samples } => {
                assert!(rpc_url.is_none());
                assert_eq!(samples, 100);
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn parse_validate_with_options() {
        let cli = Cli::try_parse_from([
            "evm-state",
            "validate",
            "--rpc-url",
            "https://polygon-rpc.com",
            "--samples",
            "50",
        ])
        .unwrap();

        match cli.command {
            Command::Validate { rpc_url, samples } => {
                assert_eq!(rpc_url.as_deref(), Some("https://polygon-rpc.com"));
                assert_eq!(samples, 50);
            }
            _ => panic!("expected Validate"),
        }
    }

    #[test]
    fn parse_config_flag() {
        let cli =
            Cli::try_parse_from(["evm-state", "--config", "/etc/evm-state.toml", "serve"]).unwrap();
        assert_eq!(cli.config, Some(PathBuf::from("/etc/evm-state.toml")));
    }

    // ── Config file loading ─────────────────────────────────────────

    #[test]
    fn deserialize_full_config() {
        let toml_str = r#"
            db_path = "/data/state.mdbx"
            chain_id = 1
            log_level = "debug"
            listen = "127.0.0.1:9000"
            portal = "https://custom.sqd.dev"
            dataset = "ethereum-mainnet"
            rpc_url = "https://eth-rpc.example.com"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.db_path.as_deref(), Some("/data/state.mdbx"));
        assert_eq!(config.chain_id, Some(1));
        assert_eq!(config.log_level.as_deref(), Some("debug"));
        assert_eq!(config.listen.as_deref(), Some("127.0.0.1:9000"));
        assert_eq!(config.portal.as_deref(), Some("https://custom.sqd.dev"));
        assert_eq!(config.dataset.as_deref(), Some("ethereum-mainnet"));
        assert_eq!(config.rpc_url.as_deref(), Some("https://eth-rpc.example.com"));
    }

    #[test]
    fn deserialize_partial_config() {
        let toml_str = r#"
            chain_id = 137
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.chain_id, Some(137));
        assert!(config.db_path.is_none());
        assert!(config.listen.is_none());
    }

    #[test]
    fn deserialize_empty_config() {
        let config: Config = toml::from_str("").unwrap();
        assert!(config.db_path.is_none());
        assert!(config.chain_id.is_none());
        assert!(config.log_level.is_none());
    }

    // ── Config merge (Resolved) ─────────────────────────────────────

    #[test]
    fn resolved_uses_defaults() {
        let cli = Cli::try_parse_from(["evm-state", "serve"]).unwrap();
        let config = Config::default();
        let resolved = Resolved::from_cli_and_config(&cli, &config);

        assert_eq!(resolved.db_path, "./state.mdbx");
        assert_eq!(resolved.chain_id, 137);
        assert_eq!(resolved.log_level, "info");
    }

    #[test]
    fn resolved_config_file_overrides_defaults() {
        let cli = Cli::try_parse_from(["evm-state", "serve"]).unwrap();
        let config = Config {
            db_path: Some("/data/custom.mdbx".into()),
            chain_id: Some(1),
            log_level: Some("debug".into()),
            ..Default::default()
        };
        let resolved = Resolved::from_cli_and_config(&cli, &config);

        assert_eq!(resolved.db_path, "/data/custom.mdbx");
        assert_eq!(resolved.chain_id, 1);
        assert_eq!(resolved.log_level, "debug");
    }

    #[test]
    fn resolved_cli_overrides_config() {
        let cli = Cli::try_parse_from([
            "evm-state",
            "--db",
            "/cli/path.mdbx",
            "--chain-id",
            "1",
            "--log-level",
            "warn",
            "serve",
        ])
        .unwrap();
        let config = Config {
            db_path: Some("/config/path.mdbx".into()),
            chain_id: Some(137),
            log_level: Some("debug".into()),
            ..Default::default()
        };
        let resolved = Resolved::from_cli_and_config(&cli, &config);

        assert_eq!(resolved.db_path, "/cli/path.mdbx");
        assert_eq!(resolved.chain_id, 1);
        assert_eq!(resolved.log_level, "warn");
    }

    // ── Dataset helper ──────────────────────────────────────────────

    #[test]
    fn dataset_for_known_chains() {
        assert_eq!(
            dataset_for_chain(1).unwrap(),
            evm_state_sqd_fetcher::ETHEREUM_DATASET
        );
        assert_eq!(
            dataset_for_chain(137).unwrap(),
            evm_state_sqd_fetcher::POLYGON_DATASET
        );
    }

    #[test]
    fn dataset_for_unknown_chain_errors() {
        assert!(dataset_for_chain(999).is_err());
    }
}
