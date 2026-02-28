use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};
use evm_state_api::{AppState, build_router};
use evm_state_chain_spec::{ChainSpec, HardforkActivation, HardforkCondition};
use evm_state_common::AccountInfo;
use evm_state_db::StateDb;
use revm::primitives::hardfork::SpecId;

// ── Test harness ────────────────────────────────────────────────────

/// Start a real API server on a random port and return the base URL.
pub async fn start_server(state: Arc<AppState>) -> String {
    let router = build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    format!("http://{addr}")
}

/// Create a populated StateDb with `num_accounts` accounts, each having
/// `slots_per_account` storage slots and a deployed contract.
///
/// Returns `(TempDir, Arc<AppState>)`. The TempDir must be kept alive
/// for the duration of the test.
pub fn setup_populated_db(
    num_accounts: usize,
    slots_per_account: usize,
) -> (tempfile::TempDir, Arc<AppState>) {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(dir.path()).unwrap();

    // Deploy a simple contract to each account:
    // SLOAD(0) → MSTORE(0) → RETURN(0,32)
    let bytecode = make_sload_return_bytecode();
    let code_hash = alloy_primitives::keccak256(&bytecode);

    db.set_code(&code_hash.into(), &bytecode).unwrap();

    let batch = db.write_batch().unwrap();
    for i in 0..num_accounts {
        let addr = account_address(i);

        batch
            .set_account(
                &addr,
                &AccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: code_hash.into(),
                },
            )
            .unwrap();

        for slot_idx in 0..slots_per_account {
            let slot = B256::from(U256::from(slot_idx));
            let value = U256::from((i + 1) * 1000 + slot_idx);
            batch.set_storage(&addr, &slot, &value).unwrap();
        }
    }
    batch.set_head_block(100).unwrap();
    batch.commit().unwrap();

    let state = Arc::new(AppState {
        db,
        chain_spec: test_chain_spec(),
    });
    (dir, state)
}

/// Set up a single contract that reads `num_slots` consecutive storage slots
/// and returns the last value. This simulates a Uniswap V3 tick scan.
pub fn setup_tick_scan_db(num_slots: usize) -> (tempfile::TempDir, Arc<AppState>, Address) {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(dir.path()).unwrap();

    let bytecode = make_multi_sload_bytecode(num_slots);
    let code_hash = alloy_primitives::keccak256(&bytecode);
    let contract = account_address(0);

    db.set_code(&code_hash.into(), &bytecode).unwrap();
    db.set_account(
        &contract,
        &AccountInfo {
            nonce: 0,
            balance: U256::ZERO,
            code_hash: code_hash.into(),
        },
    )
    .unwrap();

    let batch = db.write_batch().unwrap();
    for slot_idx in 0..num_slots {
        let slot = B256::from(U256::from(slot_idx));
        let value = U256::from(slot_idx * 100 + 1);
        batch.set_storage(&contract, &slot, &value).unwrap();
    }
    batch.set_head_block(100).unwrap();
    batch.commit().unwrap();

    let state = Arc::new(AppState {
        db,
        chain_spec: test_chain_spec(),
    });
    (dir, state, contract)
}

/// Deterministic address from index (never returns the zero address).
pub fn account_address(index: usize) -> Address {
    let mut bytes = [0u8; 20];
    // Use 0xC0 prefix to avoid collision with special addresses (zero, precompiles)
    bytes[0] = 0xC0;
    let idx_bytes = (index as u64).to_be_bytes();
    bytes[12..20].copy_from_slice(&idx_bytes);
    Address::from(bytes)
}

/// Simple bytecode: SLOAD(0) → MSTORE(0) → RETURN(0, 32)
fn make_sload_return_bytecode() -> Vec<u8> {
    vec![
        0x60, 0x00, // PUSH1 0x00
        0x54, // SLOAD
        0x60, 0x00, // PUSH1 0x00
        0x52, // MSTORE
        0x60, 0x20, // PUSH1 0x20
        0x60, 0x00, // PUSH1 0x00
        0xF3, // RETURN
    ]
}

/// Bytecode that reads `num_slots` consecutive slots (0..num_slots),
/// stores the last one in memory, and returns it.
///
/// For each slot i: PUSH slot_i → SLOAD → POP (except last which goes to MSTORE → RETURN)
fn make_multi_sload_bytecode(num_slots: usize) -> Vec<u8> {
    assert!(num_slots > 0 && num_slots <= 256, "num_slots must be 1..=256");
    let mut code = Vec::new();

    for i in 0..num_slots {
        if i < 256 {
            // PUSH1 i
            code.push(0x60);
            code.push(i as u8);
        }
        // SLOAD
        code.push(0x54);

        if i < num_slots - 1 {
            // POP (discard intermediate values to keep stack clean)
            code.push(0x50);
        }
    }

    // Last SLOAD result is on stack — store and return it
    // PUSH1 0x00 → MSTORE → PUSH1 0x20 → PUSH1 0x00 → RETURN
    code.extend_from_slice(&[
        0x60, 0x00, // PUSH1 0x00
        0x52, // MSTORE
        0x60, 0x20, // PUSH1 0x20
        0x60, 0x00, // PUSH1 0x00
        0xF3, // RETURN
    ]);

    code
}

fn test_chain_spec() -> ChainSpec {
    ChainSpec {
        chain_id: 1,
        name: "test",
        hardforks: vec![HardforkActivation {
            condition: HardforkCondition::Block(0),
            spec_id: SpecId::SHANGHAI,
        }],
    }
}

// ── RPC Client (for comparison benchmarks) ──────────────────────────

/// Simple JSON-RPC client for comparing against an external Ethereum RPC.
pub struct RpcClient {
    client: ureq::Agent,
    url: String,
}

impl RpcClient {
    /// Create from the `RPC_URL` environment variable. Returns `None` if not set.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("RPC_URL").ok()?;
        if url.is_empty() {
            return None;
        }
        Some(Self {
            client: ureq::Agent::new_with_defaults(),
            url,
        })
    }

    /// `eth_getStorageAt(address, slot, "latest")`
    pub fn get_storage_at(&self, address: &Address, slot: &B256) -> Result<String, ureq::Error> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_getStorageAt",
            "params": [format!("{:#x}", address), format!("{:#x}", slot), "latest"],
            "id": 1
        });
        let resp: serde_json::Value = self
            .client
            .post(&self.url)
            .send_json(&body)?
            .into_body()
            .read_json()?;
        Ok(resp["result"].as_str().unwrap_or("0x0").to_string())
    }

    /// `eth_call({to, data}, "latest")`
    pub fn eth_call(&self, to: &Address, data: &[u8]) -> Result<String, ureq::Error> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [{
                "to": format!("{:#x}", to),
                "data": format!("0x{}", alloy_primitives::hex::encode(data)),
            }, "latest"],
            "id": 1
        });
        let resp: serde_json::Value = self
            .client
            .post(&self.url)
            .send_json(&body)?
            .into_body()
            .read_json()?;
        Ok(resp["result"].as_str().unwrap_or("0x").to_string())
    }
}

// ── Stats helpers ───────────────────────────────────────────────────

/// Compute percentile from a sorted slice of durations.
pub fn percentile(sorted: &[std::time::Duration], p: f64) -> std::time::Duration {
    if sorted.is_empty() {
        return std::time::Duration::ZERO;
    }
    let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

/// Format a duration as a human-readable string (e.g., "1.23ms", "456µs").
pub fn fmt_duration(d: std::time::Duration) -> String {
    let micros = d.as_micros();
    if micros < 1000 {
        format!("{}µs", micros)
    } else {
        format!("{:.2}ms", micros as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_creates_valid_db() {
        let (_dir, state) = setup_populated_db(5, 3);
        assert_eq!(state.db.get_head_block().unwrap(), Some(100));

        for i in 0..5 {
            let addr = account_address(i);
            let info = state.db.get_account(&addr).unwrap().unwrap();
            assert_eq!(info.nonce, 0);

            for slot_idx in 0..3 {
                let slot = B256::from(U256::from(slot_idx));
                let val = state.db.get_storage(&addr, &slot).unwrap().unwrap();
                assert_eq!(val, U256::from((i + 1) * 1000 + slot_idx));
            }
        }
    }

    #[test]
    fn tick_scan_db_has_all_slots() {
        let (_dir, state, contract) = setup_tick_scan_db(200);
        for i in 0..200 {
            let slot = B256::from(U256::from(i));
            let val = state.db.get_storage(&contract, &slot).unwrap().unwrap();
            assert_eq!(val, U256::from(i * 100 + 1));
        }
    }

    #[test]
    fn multi_sload_bytecode_valid() {
        let code = make_multi_sload_bytecode(5);
        // Should contain 5 SLOAD opcodes (0x54)
        let sload_count = code.iter().filter(|&&b| b == 0x54).count();
        assert_eq!(sload_count, 5);
        // Should end with RETURN (0xF3)
        assert_eq!(*code.last().unwrap(), 0xF3);
    }

    #[test]
    fn percentile_computation() {
        use std::time::Duration;
        let sorted: Vec<Duration> = (0..100).map(|i| Duration::from_millis(i)).collect();
        assert_eq!(percentile(&sorted, 50.0), Duration::from_millis(50));
        assert_eq!(percentile(&sorted, 99.0), Duration::from_millis(98));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_starts_and_responds() {
        let (_dir, state) = setup_populated_db(1, 1);
        let base_url = start_server(state).await;

        let resp: serde_json::Value = ureq::Agent::new_with_defaults()
            .get(&format!("{base_url}/v1/head"))
            .call()
            .unwrap()
            .into_body()
            .read_json()
            .unwrap();
        assert_eq!(resp["head_block"], 100);
    }
}
