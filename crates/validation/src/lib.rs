use alloy_primitives::{hex, Address, B256, U256};
use evm_state_common::AccountInfo;
use evm_state_db::StateDb;
use serde::Deserialize;

// ── Error type ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] evm_state_db::Error),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] ureq::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

// ── Ethereum JSON-RPC client ──────────────────────────────────────

/// Minimal synchronous Ethereum JSON-RPC client.
pub struct EthRpcClient {
    client: ureq::Agent,
    rpc_url: String,
}

impl EthRpcClient {
    pub fn new(rpc_url: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build();
        Self {
            client: config.into(),
            rpc_url: rpc_url.to_string(),
        }
    }

    /// `eth_getBalance(address, block)`
    pub fn get_balance(&self, address: &Address, block: u64) -> Result<U256> {
        let result = self.call("eth_getBalance", &[
            format!("\"0x{address:x}\""),
            format!("\"0x{block:x}\""),
        ])?;
        parse_u256_hex(&result)
    }

    /// `eth_getTransactionCount(address, block)`
    pub fn get_nonce(&self, address: &Address, block: u64) -> Result<u64> {
        let result = self.call("eth_getTransactionCount", &[
            format!("\"0x{address:x}\""),
            format!("\"0x{block:x}\""),
        ])?;
        parse_u64_hex(&result)
    }

    /// `eth_getCode(address, block)`
    pub fn get_code(&self, address: &Address, block: u64) -> Result<Vec<u8>> {
        let result = self.call("eth_getCode", &[
            format!("\"0x{address:x}\""),
            format!("\"0x{block:x}\""),
        ])?;
        let s = result
            .as_str()
            .ok_or_else(|| Error::Rpc("expected string result".to_string()))?;
        hex::decode(s).map_err(|e| Error::Rpc(format!("invalid hex in code: {e}")))
    }

    /// `eth_getStorageAt(address, slot, block)`
    pub fn get_storage_at(
        &self,
        address: &Address,
        slot: &B256,
        block: u64,
    ) -> Result<U256> {
        let result = self.call("eth_getStorageAt", &[
            format!("\"0x{address:x}\""),
            format!("\"0x{slot}\""),
            format!("\"0x{block:x}\""),
        ])?;
        parse_u256_hex(&result)
    }

    fn call(&self, method: &str, params: &[String]) -> Result<serde_json::Value> {
        let params_str = params.join(",");
        let body_str = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":[{params_str}]}}"#
        );
        let body: serde_json::Value = serde_json::from_str(&body_str)?;

        let response = self
            .client
            .post(&self.rpc_url)
            .send_json(&body)?;

        let resp: serde_json::Value = response.into_body().read_json()?;

        if let Some(error) = resp.get("error") {
            let msg: &str = error
                .get("message")
                .and_then(|m: &serde_json::Value| m.as_str())
                .unwrap_or("unknown RPC error");
            return Err(Error::Rpc(msg.to_string()));
        }

        resp.get("result")
            .cloned()
            .ok_or_else(|| Error::Rpc("missing 'result' in JSON-RPC response".to_string()))
    }
}

// ── Hex parsing helpers ───────────────────────────────────────────

fn parse_u256_hex(value: &serde_json::Value) -> Result<U256> {
    let s = value
        .as_str()
        .ok_or_else(|| Error::Rpc("expected string result".to_string()))?;
    let s = s.strip_prefix("0x").unwrap_or(s);
    U256::from_str_radix(s, 16).map_err(|e| Error::Rpc(format!("invalid U256 hex: {e}")))
}

fn parse_u64_hex(value: &serde_json::Value) -> Result<u64> {
    let s = value
        .as_str()
        .ok_or_else(|| Error::Rpc("expected string result".to_string()))?;
    let s = s.strip_prefix("0x").unwrap_or(s);
    u64::from_str_radix(s, 16).map_err(|e| Error::Rpc(format!("invalid u64 hex: {e}")))
}

// ── Validation report types ───────────────────────────────────────

/// Result of comparing a single account field.
#[derive(Debug, Clone)]
pub struct AccountCheck {
    pub address: Address,
    pub field: &'static str,
    pub local: String,
    pub remote: String,
    pub matches: bool,
}

/// Result of comparing a single storage slot.
#[derive(Debug, Clone)]
pub struct StorageCheck {
    pub address: Address,
    pub slot: B256,
    pub local: U256,
    pub remote: U256,
    pub matches: bool,
}

/// Summary of a validation run.
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub block: u64,
    pub account_checks: Vec<AccountCheck>,
    pub storage_checks: Vec<StorageCheck>,
}

impl ValidationReport {
    pub fn total_checks(&self) -> usize {
        self.account_checks.len() + self.storage_checks.len()
    }

    pub fn mismatches(&self) -> usize {
        self.account_checks.iter().filter(|c| !c.matches).count()
            + self.storage_checks.iter().filter(|c| !c.matches).count()
    }

    pub fn is_valid(&self) -> bool {
        self.mismatches() == 0
    }
}

// ── Validation functions ──────────────────────────────────────────

/// Sample up to `count` random accounts and `count` random storage slots
/// from the local DB, compare against the remote RPC at `block`, and
/// return a report.
pub fn validate_random(
    db: &StateDb,
    rpc: &EthRpcClient,
    count: usize,
    block: u64,
) -> Result<ValidationReport> {
    let all_accounts = db.iter_accounts()?;
    let all_storage = db.iter_storage()?;

    // Deterministic sampling: take evenly spaced entries.
    let sampled_accounts = sample(&all_accounts, count);
    let sampled_storage = sample(&all_storage, count);

    let mut account_checks = Vec::new();
    for (address, info) in sampled_accounts {
        check_account(rpc, address, info, block, &mut account_checks)?;
    }

    let mut storage_checks = Vec::new();
    for (address, slot, local_value) in sampled_storage {
        let remote_value = rpc.get_storage_at(address, slot, block)?;
        storage_checks.push(StorageCheck {
            address: *address,
            slot: *slot,
            local: *local_value,
            remote: remote_value,
            matches: *local_value == remote_value,
        });
    }

    Ok(ValidationReport {
        block,
        account_checks,
        storage_checks,
    })
}

fn check_account(
    rpc: &EthRpcClient,
    address: &Address,
    info: &AccountInfo,
    block: u64,
    checks: &mut Vec<AccountCheck>,
) -> Result<()> {
    // Nonce
    let remote_nonce = rpc.get_nonce(address, block)?;
    checks.push(AccountCheck {
        address: *address,
        field: "nonce",
        local: info.nonce.to_string(),
        remote: remote_nonce.to_string(),
        matches: info.nonce == remote_nonce,
    });

    // Balance
    let remote_balance = rpc.get_balance(address, block)?;
    checks.push(AccountCheck {
        address: *address,
        field: "balance",
        local: format!("0x{:x}", info.balance),
        remote: format!("0x{:x}", remote_balance),
        matches: info.balance == remote_balance,
    });

    // Code hash — compare by fetching code and hashing
    let remote_code = rpc.get_code(address, block)?;
    let remote_code_hash = if remote_code.is_empty() {
        B256::ZERO
    } else {
        alloy_primitives::keccak256(&remote_code).into()
    };
    checks.push(AccountCheck {
        address: *address,
        field: "codeHash",
        local: format!("0x{}", info.code_hash),
        remote: format!("0x{}", remote_code_hash),
        matches: info.code_hash == remote_code_hash,
    });

    Ok(())
}

/// Binary search for the first block where the remote RPC's value for
/// `(address, slot)` diverges from `local_value`.
///
/// Returns `None` if the remote value matches `local_value` at `high_block`
/// (no divergence). Returns `Some(low_block)` if the remote already differs
/// at `low_block`.
pub fn find_divergence_block(
    rpc: &EthRpcClient,
    address: &Address,
    slot: &B256,
    local_value: U256,
    low_block: u64,
    high_block: u64,
) -> Result<Option<u64>> {
    // No divergence: remote matches local at the high block.
    let remote_high = rpc.get_storage_at(address, slot, high_block)?;
    if remote_high == local_value {
        return Ok(None);
    }

    // Check if divergence is already present at the low block.
    let remote_low = rpc.get_storage_at(address, slot, low_block)?;
    if remote_low != local_value {
        return Ok(Some(low_block));
    }

    // Binary search: find first block where remote != local_value.
    let mut lo = low_block;
    let mut hi = high_block;

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let remote_mid = rpc.get_storage_at(address, slot, mid)?;

        if remote_mid == local_value {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }

    Ok(Some(lo))
}

/// Take up to `count` evenly spaced samples from a slice.
fn sample<T>(items: &[T], count: usize) -> Vec<&T> {
    if items.is_empty() || count == 0 {
        return Vec::new();
    }
    if count >= items.len() {
        return items.iter().collect();
    }
    let step = items.len() as f64 / count as f64;
    (0..count)
        .map(|i| &items[(i as f64 * step) as usize])
        .collect()
}

// ── JSON-RPC request/response types (for mock server tests) ───────

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: serde_json::Value,
    method: String,
    params: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};
    use axum::extract::State;
    use axum::routing::post;
    use axum::Json;
    use std::collections::HashMap;
    use std::sync::Arc;

    // ── Mock RPC server ──────────────────────────────────────────

    #[derive(Clone)]
    struct MockState {
        /// (method, params_hash) → result value
        responses: Arc<HashMap<String, serde_json::Value>>,
    }

    fn response_key(method: &str, params: &[serde_json::Value]) -> String {
        format!("{}:{}", method, serde_json::to_string(params).unwrap())
    }

    async fn mock_rpc_handler(
        State(state): State<MockState>,
        Json(req): Json<JsonRpcRequest>,
    ) -> Json<serde_json::Value> {
        let key = response_key(&req.method, &req.params);
        let result = state
            .responses
            .get(&key)
            .cloned()
            .unwrap_or(serde_json::Value::String("0x0".to_string()));

        Json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": req.id,
            "result": result
        }))
    }

    fn start_mock_rpc(
        responses: HashMap<String, serde_json::Value>,
    ) -> (String, tokio::runtime::Runtime) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let state = MockState {
            responses: Arc::new(responses),
        };
        let app = axum::Router::new()
            .route("/", post(mock_rpc_handler))
            .with_state(state);

        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{addr}");

        rt.spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        (url, rt)
    }

    fn tmp_db() -> (tempfile::TempDir, StateDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path()).unwrap();
        (dir, db)
    }

    // ── RPC client tests ─────────────────────────────────────────

    #[test]
    fn rpc_get_balance() {
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let block = 100u64;

        let mut responses = HashMap::new();
        responses.insert(
            response_key(
                "eth_getBalance",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String("0x3e8".to_string()),
        );

        let (url, _rt) = start_mock_rpc(responses);
        let rpc = EthRpcClient::new(&url);
        let balance = rpc.get_balance(&addr, block).unwrap();
        assert_eq!(balance, U256::from(0x3e8u64));
    }

    #[test]
    fn rpc_get_storage_at() {
        let addr = address!("0000000000000000000000000000000000c0ffee");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        let block = 200u64;

        let mut responses = HashMap::new();
        responses.insert(
            response_key(
                "eth_getStorageAt",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{slot}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String(
                "0x000000000000000000000000000000000000000000000000000000000000002a"
                    .to_string(),
            ),
        );

        let (url, _rt) = start_mock_rpc(responses);
        let rpc = EthRpcClient::new(&url);
        let value = rpc.get_storage_at(&addr, &slot, block).unwrap();
        assert_eq!(value, U256::from(0x2au64));
    }

    // ── Validation tests ─────────────────────────────────────────

    #[test]
    fn validate_random_all_match() {
        let (_dir, db) = tmp_db();
        let addr = address!("0000000000000000000000000000000000000001");
        let slot = B256::ZERO;
        let block = 100u64;

        let info = AccountInfo {
            nonce: 5,
            balance: U256::from(1000u64),
            code_hash: B256::ZERO,
        };
        db.set_account(&addr, &info).unwrap();
        db.set_storage(&addr, &slot, &U256::from(42u64)).unwrap();

        let mut responses = HashMap::new();
        // Nonce
        responses.insert(
            response_key(
                "eth_getTransactionCount",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String("0x5".to_string()),
        );
        // Balance
        responses.insert(
            response_key(
                "eth_getBalance",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String("0x3e8".to_string()),
        );
        // Code (empty → B256::ZERO code hash)
        responses.insert(
            response_key(
                "eth_getCode",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String("0x".to_string()),
        );
        // Storage
        responses.insert(
            response_key(
                "eth_getStorageAt",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{slot}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String(
                "0x000000000000000000000000000000000000000000000000000000000000002a"
                    .to_string(),
            ),
        );

        let (url, _rt) = start_mock_rpc(responses);
        let rpc = EthRpcClient::new(&url);

        let report = validate_random(&db, &rpc, 100, block).unwrap();
        assert!(report.is_valid());
        assert_eq!(report.account_checks.len(), 3); // nonce, balance, codeHash
        assert_eq!(report.storage_checks.len(), 1);
        assert_eq!(report.mismatches(), 0);
    }

    #[test]
    fn validate_random_detects_mismatch() {
        let (_dir, db) = tmp_db();
        let addr = address!("0000000000000000000000000000000000000001");
        let slot = B256::ZERO;
        let block = 100u64;

        let info = AccountInfo {
            nonce: 5,
            balance: U256::from(1000u64),
            code_hash: B256::ZERO,
        };
        db.set_account(&addr, &info).unwrap();
        db.set_storage(&addr, &slot, &U256::from(42u64)).unwrap();

        let mut responses = HashMap::new();
        responses.insert(
            response_key(
                "eth_getTransactionCount",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String("0x5".to_string()),
        );
        responses.insert(
            response_key(
                "eth_getBalance",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            // Different balance!
            serde_json::Value::String("0x999".to_string()),
        );
        responses.insert(
            response_key(
                "eth_getCode",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            serde_json::Value::String("0x".to_string()),
        );
        responses.insert(
            response_key(
                "eth_getStorageAt",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{slot}")),
                    serde_json::Value::String(format!("0x{block:x}")),
                ],
            ),
            // Different storage value!
            serde_json::Value::String(
                "0x0000000000000000000000000000000000000000000000000000000000000099"
                    .to_string(),
            ),
        );

        let (url, _rt) = start_mock_rpc(responses);
        let rpc = EthRpcClient::new(&url);

        let report = validate_random(&db, &rpc, 100, block).unwrap();
        assert!(!report.is_valid());
        assert_eq!(report.mismatches(), 2); // balance + storage

        // Check specific mismatches.
        let balance_check = report
            .account_checks
            .iter()
            .find(|c| c.field == "balance")
            .unwrap();
        assert!(!balance_check.matches);

        let storage_check = &report.storage_checks[0];
        assert!(!storage_check.matches);
        assert_eq!(storage_check.local, U256::from(42u64));
        assert_eq!(storage_check.remote, U256::from(0x99u64));
    }

    // ── Binary search tests ──────────────────────────────────────

    #[test]
    fn find_divergence_block_locates_exact_block() {
        let addr = address!("0000000000000000000000000000000000c0ffee");
        let slot = B256::ZERO;
        let local_value = U256::from(100u64);
        let diverged_value = U256::from(999u64);
        let diverge_at = 51u64;

        // Blocks 1-50: value matches local (100)
        // Blocks 51-100: value differs (999)
        let mut responses = HashMap::new();
        for block in 1u64..=100 {
            let val = if block < diverge_at {
                local_value
            } else {
                diverged_value
            };
            responses.insert(
                response_key(
                    "eth_getStorageAt",
                    &[
                        serde_json::Value::String(format!("0x{addr:x}")),
                        serde_json::Value::String(format!("0x{slot}")),
                        serde_json::Value::String(format!("0x{block:x}")),
                    ],
                ),
                serde_json::Value::String(format!(
                    "0x{:064x}",
                    val
                )),
            );
        }

        let (url, _rt) = start_mock_rpc(responses);
        let rpc = EthRpcClient::new(&url);

        let result =
            find_divergence_block(&rpc, &addr, &slot, local_value, 1, 100).unwrap();
        assert_eq!(result, Some(51));
    }

    #[test]
    fn find_divergence_block_no_divergence() {
        let addr = address!("0000000000000000000000000000000000c0ffee");
        let slot = B256::ZERO;
        let local_value = U256::from(42u64);

        let mut responses = HashMap::new();
        // Remote matches local at high block.
        responses.insert(
            response_key(
                "eth_getStorageAt",
                &[
                    serde_json::Value::String(format!("0x{addr:x}")),
                    serde_json::Value::String(format!("0x{slot}")),
                    serde_json::Value::String("0x64".to_string()), // block 100
                ],
            ),
            serde_json::Value::String(format!(
                "0x{:064x}",
                local_value
            )),
        );

        let (url, _rt) = start_mock_rpc(responses);
        let rpc = EthRpcClient::new(&url);

        let result =
            find_divergence_block(&rpc, &addr, &slot, local_value, 1, 100).unwrap();
        assert_eq!(result, None);
    }
}
