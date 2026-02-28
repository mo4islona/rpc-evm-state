use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};
use anyhow::{Context, Result, bail};
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

// ── Genesis file types ─────────────────────────────────────────────

/// Standard Geth/Bor genesis.json format (only the fields we need).
#[derive(Debug, Deserialize)]
struct GenesisFile {
    alloc: HashMap<Address, GenesisAccount>,
}

/// A single account entry in the genesis `alloc` section.
#[derive(Debug, Deserialize)]
struct GenesisAccount {
    #[serde(default, deserialize_with = "deserialize_balance")]
    balance: Option<U256>,
    #[serde(default, deserialize_with = "deserialize_optional_code")]
    code: Option<Vec<u8>>,
    #[serde(default)]
    storage: Option<HashMap<B256, U256>>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_hex")]
    nonce: Option<u64>,
}

fn deserialize_balance<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<U256>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    match s {
        None => Ok(None),
        Some(s) => {
            let s = s.trim();
            if s.is_empty() || s == "0" || s == "0x0" {
                return Ok(Some(U256::ZERO));
            }
            // Geth uses "0x..." hex strings for balance
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                U256::from_str_radix(hex, 16)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            } else {
                // Decimal string
                U256::from_str_radix(&s, 10)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
        }
    }
}

fn deserialize_optional_code<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<Vec<u8>>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    match s {
        None => Ok(None),
        Some(s) => {
            let hex = s.strip_prefix("0x").unwrap_or(&s);
            hex::decode(hex).map(Some).map_err(serde::de::Error::custom)
        }
    }
}

fn deserialize_optional_u64_hex<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<u64>, D::Error> {
    let val: Option<serde_json::Value> = Option::deserialize(d)?;
    match val {
        None => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_u64()),
        Some(serde_json::Value::String(s)) => {
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                u64::from_str_radix(hex, 16)
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            } else {
                s.parse::<u64>().map(Some).map_err(serde::de::Error::custom)
            }
        }
        _ => Err(serde::de::Error::custom(
            "expected number or hex string for nonce",
        )),
    }
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

        /// Use plain text logging instead of the TUI progress bar.
        #[arg(long)]
        plain: bool,
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

    /// Import genesis allocations from a genesis.json file.
    Genesis {
        /// Path to genesis.json (Geth/Bor format).
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
            chain_id: cli.chain_id.or(config.chain_id).unwrap_or(137),
            log_level: cli
                .log_level
                .clone()
                .or_else(|| config.log_level.clone())
                .unwrap_or_else(|| "info".to_string()),
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Format a number with thousand separators: `1234567` → `"1,234,567"`.
fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result
}

/// Format a duration as a human-readable ETA string.
/// Examples: "32s", "5m 12s", "1h 23m", "3d 5h"
fn fmt_eta(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else if secs < 86400 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{}d {}h", secs / 86400, (secs % 86400) / 3600)
    }
}

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
    plain: bool,
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

    let use_state_diffs = chain_spec.requires_state_diffs;
    let mode_label = if use_state_diffs {
        "state-diffs"
    } else {
        "replay"
    };
    let chain_spec_name = chain_spec.name;

    let fetcher = evm_state_sqd_fetcher::SqdFetcher::new(&portal_url, &dataset_name)
        .with_state_diffs(use_state_diffs);

    // Determine the target block for ETA calculation.
    // Use --to if specified, otherwise query the portal for the latest available block.
    let target_block = match to {
        Some(t) => Some(t),
        None => fetcher.get_head_block().await.unwrap_or(None),
    };

    let target_label = target_block
        .map(|t| fmt_num(t))
        .unwrap_or_else(|| "∞".into());

    let blocks = fetcher.stream_blocks(start_block, to);

    let pipeline_mode = if use_state_diffs {
        evm_state_replayer::pipeline::PipelineMode::StateDiffs
    } else {
        evm_state_replayer::pipeline::PipelineMode::Replay(chain_spec)
    };

    if plain {
        // ── Plain text logging ──────────────────────────────────────
        info!(
            chain = chain_spec_name,
            chain_id = resolved.chain_id,
            mode = mode_label,
            from = start_block,
            to = %target_label,
            "starting replay"
        );

        let log_interval = std::time::Duration::from_secs(5);
        let mut last_log = std::time::Instant::now();

        let stats =
            evm_state_replayer::pipeline::run_pipeline(&db, &pipeline_mode, blocks, |progress| {
                if last_log.elapsed() >= log_interval {
                    last_log = std::time::Instant::now();
                    let bps = progress.blocks_per_sec();
                    let eta = if let Some(target) = target_block {
                        if bps > 0.0 && progress.block_number < target {
                            let remaining = target - progress.block_number;
                            fmt_eta(std::time::Duration::from_secs_f64(remaining as f64 / bps))
                        } else {
                            "-".into()
                        }
                    } else {
                        "-".into()
                    };
                    let pct = target_block.map(|t| {
                        let total = t.saturating_sub(start_block);
                        if total > 0 {
                            ((progress.block_number.saturating_sub(start_block)) as f64
                                / total as f64
                                * 100.0) as u64
                        } else {
                            0
                        }
                    });
                    info!(
                        block = %format!("{}/{}", fmt_num(progress.block_number), target_label),
                        bps = %fmt_num(bps as u64),
                        eta = %eta,
                        pct = %pct.map(|p| format!("{p}%")).unwrap_or_else(|| "-".into()),
                        "replay progress"
                    );
                }
                true
            })
            .await?;

        info!(
            blocks = %fmt_num(stats.blocks_processed),
            elapsed = %fmt_eta(stats.elapsed),
            bps = %fmt_num(stats.blocks_per_sec() as u64),
            "replay complete"
        );
    } else {
        // ── TUI mode ────────────────────────────────────────────────
        eprintln!();
        eprintln!("  \x1b[36m███████╗ ██████╗ ██████╗ \x1b[0m");
        eprintln!("  \x1b[36m██╔════╝██╔═══██╗██╔══██╗\x1b[0m");
        eprintln!("  \x1b[36m███████╗██║   ██║██║  ██║\x1b[0m");
        eprintln!("  \x1b[36m╚════██║██║▄▄ ██║██║  ██║\x1b[0m");
        eprintln!("  \x1b[36m███████║╚██████╔╝██████╔╝\x1b[0m");
        eprintln!("  \x1b[36m╚══════╝ ╚══▀▀═╝ ╚═════╝\x1b[0m");
        eprintln!();
        eprintln!("  EVM State Replayer");
        eprintln!();
        eprintln!(
            "  chain: \x1b[1m{}\x1b[0m ({})  mode: \x1b[1m{}\x1b[0m",
            chain_spec_name, resolved.chain_id, mode_label
        );
        eprintln!(
            "  from block \x1b[1m{}\x1b[0m to \x1b[1m{}\x1b[0m",
            fmt_num(start_block),
            target_label
        );
        eprintln!();

        let total = target_block
            .map(|t| t.saturating_sub(start_block))
            .unwrap_or(0);

        let pb = if total > 0 {
            indicatif::ProgressBar::new(total)
        } else {
            indicatif::ProgressBar::new_spinner()
        };

        pb.set_style(
            indicatif::ProgressStyle::with_template(if total > 0 {
                "  {spinner:.cyan} [{bar:40.cyan/dim}] {percent}%\n  {msg}"
            } else {
                "  {spinner:.cyan} {msg}"
            })
            .unwrap()
            .progress_chars("██░")
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", " "]),
        );
        pb.enable_steady_tick(std::time::Duration::from_millis(120));

        let stats =
            evm_state_replayer::pipeline::run_pipeline(&db, &pipeline_mode, blocks, |progress| {
                let done = progress.block_number.saturating_sub(start_block);
                pb.set_position(done);

                let bps = progress.blocks_per_sec();
                let eta = if let Some(target) = target_block {
                    if bps > 0.0 && progress.block_number < target {
                        let remaining = target - progress.block_number;
                        fmt_eta(std::time::Duration::from_secs_f64(remaining as f64 / bps))
                    } else {
                        "-".into()
                    }
                } else {
                    "-".into()
                };

                pb.set_message(format!(
                    "Block {}/{} | {} blocks/sec | ETA {}",
                    fmt_num(progress.block_number),
                    target_label,
                    fmt_num(bps as u64),
                    eta,
                ));
                true
            })
            .await?;

        pb.finish_and_clear();
        eprintln!(
            "  \x1b[32m✓\x1b[0m Done! \x1b[1m{}\x1b[0m blocks in \x1b[1m{}\x1b[0m (\x1b[1m{}\x1b[0m bps)",
            fmt_num(stats.blocks_processed),
            fmt_eta(stats.elapsed),
            fmt_num(stats.blocks_per_sec() as u64),
        );
        eprintln!();
    }

    Ok(())
}

fn run_import(resolved: &Resolved, file: &std::path::Path) -> Result<()> {
    let db = evm_state_db::StateDb::open(&resolved.db_path)
        .with_context(|| format!("failed to open database at {}", resolved.db_path))?;

    info!(file = %file.display(), db = %resolved.db_path, "importing snapshot");

    let stats = evm_state_snapshot::import_snapshot(&db, file, |progress| {
        if progress.entries_imported % 100_000 == 0 && progress.entries_imported > 0 {
            info!(
                entries = %fmt_num(progress.entries_imported as u64),
                bytes = %fmt_num(progress.bytes_read),
                "import progress"
            );
        }
    })?;

    info!(
        accounts = %fmt_num(stats.accounts as u64),
        storage_slots = %fmt_num(stats.storage_slots as u64),
        code_entries = %fmt_num(stats.code_entries as u64),
        head_block = %fmt_num(stats.head_block.unwrap_or(0)),
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
                entries = %fmt_num(progress.entries_written as u64),
                "export progress"
            );
        }
    })?;

    info!(
        accounts = %fmt_num(stats.accounts as u64),
        storage_slots = %fmt_num(stats.storage_slots as u64),
        code_entries = %fmt_num(stats.code_entries as u64),
        head_block = %fmt_num(stats.head_block.unwrap_or(0)),
        elapsed = ?stats.elapsed,
        "export complete"
    );

    Ok(())
}

fn run_genesis(resolved: &Resolved, file: &std::path::Path) -> Result<()> {
    let contents = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read genesis file: {}", file.display()))?;
    let genesis: GenesisFile = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse genesis file: {}", file.display()))?;

    let db = evm_state_db::StateDb::open(&resolved.db_path)
        .with_context(|| format!("failed to open database at {}", resolved.db_path))?;

    info!(
        file = %file.display(),
        accounts = %fmt_num(genesis.alloc.len() as u64),
        "importing genesis allocations"
    );

    let batch = db.write_batch()?;
    let mut accounts = 0u64;
    let mut storage_slots = 0u64;
    let mut code_entries = 0u64;

    for (address, account) in &genesis.alloc {
        let code_hash = if let Some(code) = &account.code {
            let hash = alloy_primitives::keccak256(code);
            if !code.is_empty() {
                batch.set_code(&hash.into(), code)?;
                code_entries += 1;
            }
            hash.into()
        } else {
            B256::ZERO
        };

        let info = evm_state_common::AccountInfo {
            nonce: account.nonce.unwrap_or(0),
            balance: account.balance.unwrap_or(U256::ZERO),
            code_hash,
        };
        batch.set_account(address, &info)?;
        accounts += 1;

        if let Some(storage) = &account.storage {
            for (slot, value) in storage {
                batch.set_storage(address, slot, value)?;
                storage_slots += 1;
            }
        }
    }

    batch.set_head_block(0)?;
    batch.commit()?;

    info!(
        accounts = %fmt_num(accounts),
        storage_slots = %fmt_num(storage_slots),
        code_entries = %fmt_num(code_entries),
        "genesis import complete"
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

    info!(rpc = %rpc_url, block = %fmt_num(head), samples = %fmt_num(samples as u64), "starting validation");

    let rpc = evm_state_validation::EthRpcClient::new(&rpc_url);
    let report = evm_state_validation::validate_random(&db, &rpc, samples, head)?;

    info!(
        total = %fmt_num(report.total_checks() as u64),
        mismatches = %fmt_num(report.mismatches() as u64),
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
            plain,
        } => run_replay(&resolved, &config, portal, dataset, from, to, plain).await,
        Command::Import { file } => run_import(&resolved, &file),
        Command::Export { file } => run_export(&resolved, &file),
        Command::Genesis { file } => run_genesis(&resolved, &file),
        Command::Validate { rpc_url, samples } => {
            run_validate(&resolved, &config, rpc_url, samples)
        }
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
                plain,
            } => {
                assert!(portal.is_none());
                assert!(dataset.is_none());
                assert!(from.is_none());
                assert!(to.is_none());
                assert!(!plain);
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
                ..
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
    fn parse_genesis() {
        let cli = Cli::try_parse_from(["evm-state", "genesis", "/tmp/genesis.json"]).unwrap();
        match cli.command {
            Command::Genesis { file } => assert_eq!(file, PathBuf::from("/tmp/genesis.json")),
            _ => panic!("expected Genesis"),
        }
    }

    #[test]
    fn parse_genesis_missing_file_fails() {
        assert!(Cli::try_parse_from(["evm-state", "genesis"]).is_err());
    }

    #[test]
    fn deserialize_genesis_file() {
        let json = r#"{
            "alloc": {
                "0000000000000000000000000000000000001010": {
                    "balance": "0x204fcce2c5a141f7f9a00000",
                    "code": "0x6080"
                },
                "5973918275C01F50555d44e92c9d9b353CaDAD54": {
                    "balance": "0x3635c9adc5dea00000"
                }
            }
        }"#;
        let genesis: GenesisFile = serde_json::from_str(json).unwrap();
        assert_eq!(genesis.alloc.len(), 2);

        let matic = genesis
            .alloc
            .iter()
            .find(|(a, _)| a.to_string().contains("1010"))
            .unwrap()
            .1;
        assert!(matic.balance.unwrap() > U256::ZERO);
        assert_eq!(matic.code.as_ref().unwrap(), &[0x60, 0x80]);

        let funded = genesis
            .alloc
            .iter()
            .find(|(a, _)| a.to_string().to_lowercase().contains("5973"))
            .unwrap()
            .1;
        assert!(funded.balance.unwrap() > U256::ZERO);
        assert!(funded.code.is_none());
    }

    #[test]
    fn deserialize_genesis_with_storage_and_nonce() {
        let json = r#"{
            "alloc": {
                "0000000000000000000000000000000000000001": {
                    "balance": "0x100",
                    "nonce": "0x5",
                    "storage": {
                        "0x0000000000000000000000000000000000000000000000000000000000000001": "0x0000000000000000000000000000000000000000000000000000000000000042"
                    }
                }
            }
        }"#;
        let genesis: GenesisFile = serde_json::from_str(json).unwrap();
        assert_eq!(genesis.alloc.len(), 1);

        let (_, account) = genesis.alloc.iter().next().unwrap();
        assert_eq!(account.balance.unwrap(), U256::from(0x100));
        assert_eq!(account.nonce.unwrap(), 5);
        let storage = account.storage.as_ref().unwrap();
        assert_eq!(storage.len(), 1);
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
        assert_eq!(
            config.rpc_url.as_deref(),
            Some("https://eth-rpc.example.com")
        );
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
