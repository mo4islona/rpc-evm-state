//! Integration test: replay Polygon blocks 0–2000 from the real SQD portal.
//!
//! Requires network access. Run with:
//!   cargo test -p evm-state-replayer --test polygon_portal -- --ignored --nocapture

use alloy_primitives::{keccak256, B256, U256};
use evm_state_chain_spec::{from_chain_id, ChainSpec};
use evm_state_common::AccountInfo;
use evm_state_db::StateDb;
use evm_state_replayer::pipeline::{run_pipeline, PipelineMode};
use evm_state_sqd_fetcher::SqdFetcher;
use serde::Deserialize;
use std::collections::HashMap;

// ── Genesis parsing (mirrors crates/cli) ─────────────────────────────

#[derive(Deserialize)]
struct GenesisFile {
    alloc: HashMap<alloy_primitives::Address, GenesisAccount>,
}

#[derive(Deserialize)]
struct GenesisAccount {
    #[serde(default, deserialize_with = "deser_balance")]
    balance: Option<U256>,
    #[serde(default, deserialize_with = "deser_code")]
    code: Option<Vec<u8>>,
    #[serde(default)]
    storage: Option<HashMap<B256, U256>>,
    #[serde(default, deserialize_with = "deser_nonce")]
    nonce: Option<u64>,
}

fn deser_balance<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<U256>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    match s {
        None => Ok(None),
        Some(s) => {
            let s = s.trim();
            if s.is_empty() || s == "0" || s == "0x0" {
                return Ok(Some(U256::ZERO));
            }
            if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                U256::from_str_radix(hex, 16).map(Some).map_err(serde::de::Error::custom)
            } else {
                U256::from_str_radix(&s, 10).map(Some).map_err(serde::de::Error::custom)
            }
        }
    }
}

fn deser_code<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    match s {
        None => Ok(None),
        Some(s) => {
            let hex = s.strip_prefix("0x").unwrap_or(&s);
            alloy_primitives::hex::decode(hex).map(Some).map_err(serde::de::Error::custom)
        }
    }
}

fn deser_nonce<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    match s {
        None => Ok(None),
        Some(s) => {
            let hex = s.strip_prefix("0x").unwrap_or(&s);
            u64::from_str_radix(hex, 16).map(Some).map_err(serde::de::Error::custom)
        }
    }
}

fn import_genesis(db: &StateDb, genesis: &GenesisFile) {
    let batch = db.write_batch().unwrap();
    for (address, account) in &genesis.alloc {
        let code_hash = if let Some(code) = &account.code {
            let hash = keccak256(code);
            if !code.is_empty() {
                batch.set_code(&hash.into(), code).unwrap();
            }
            hash.into()
        } else {
            B256::ZERO
        };

        let info = AccountInfo {
            nonce: account.nonce.unwrap_or(0),
            balance: account.balance.unwrap_or(U256::ZERO),
            code_hash,
        };
        batch.set_account(address, &info).unwrap();

        if let Some(storage) = &account.storage {
            for (slot, value) in storage {
                batch.set_storage(address, slot, value).unwrap();
            }
        }
    }
    batch.commit().unwrap();
}

// ── Test ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore] // requires network + genesis.json — run with: cargo test -p evm-state-replayer --test polygon_portal -- --ignored --nocapture
async fn replay_polygon_first_2000_blocks() {
    // 1. Load genesis
    let genesis_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("genesis.json");
    assert!(
        genesis_path.exists(),
        "genesis.json not found at {}. Download it first:\n  \
         curl -Lo genesis.json https://raw.githubusercontent.com/maticnetwork/bor/develop/builder/files/genesis-mainnet-v1.json",
        genesis_path.display()
    );

    let genesis: GenesisFile =
        serde_json::from_str(&std::fs::read_to_string(&genesis_path).unwrap())
            .expect("failed to parse genesis.json");

    // 2. Create temp DB and import genesis
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(dir.path()).unwrap();
    import_genesis(&db, &genesis);

    eprintln!(
        "imported {} genesis accounts",
        genesis.alloc.len()
    );

    // 3. Get Polygon chain spec
    let chain_spec: ChainSpec = from_chain_id(137).expect("polygon chain spec");
    assert!(chain_spec.disable_balance_check);

    // 4. Stream blocks 0–2000 from real portal
    let fetcher = SqdFetcher::new(
        evm_state_sqd_fetcher::DEFAULT_PORTAL,
        evm_state_sqd_fetcher::POLYGON_DATASET,
    );

    let blocks = fetcher.stream_blocks(0, Some(2000));

    // 5. Replay
    let stats = run_pipeline(&db, &PipelineMode::Replay(chain_spec), blocks, |p| {
        if p.blocks_processed % 500 == 0 {
            eprintln!(
                "block {} ({} blocks, {:.0} blk/s)",
                p.block_number,
                p.blocks_processed,
                p.blocks_per_sec()
            );
        }
        true
    })
    .await
    .expect("replay should succeed for blocks 0–2000");

    eprintln!(
        "replayed {} blocks in {:.1}s ({:.0} blk/s)",
        stats.blocks_processed,
        stats.elapsed.as_secs_f64(),
        stats.blocks_per_sec()
    );

    // 6. Verify
    assert_eq!(stats.blocks_processed, 2001); // blocks 0 through 2000 inclusive
    assert_eq!(stats.first_block, Some(0));
    assert_eq!(stats.last_block, Some(2000));
    assert_eq!(db.get_head_block().unwrap(), Some(2000));
}
