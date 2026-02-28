use std::num::NonZeroUsize;
use std::sync::Mutex;

use alloy_primitives::{hex, Address, Bytes, B256, U256};
use lru::LruCache;
use revm::bytecode::Bytecode;
use revm::database_interface::{DBErrorMarker, DatabaseRef};
use revm::state::AccountInfo;
use serde::{Deserialize, Serialize};

// ── Error type ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] ureq::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("server error: HTTP {0}")]
    Server(u16),

    #[error("invalid response: {0}")]
    InvalidResponse(String),

    #[error("EVM execution error: {0}")]
    Evm(String),
}

impl DBErrorMarker for Error {}

// ── Cache structures ───────────────────────────────────────────────

#[derive(Clone)]
struct CachedAccount {
    nonce: u64,
    balance: U256,
    code_hash: B256,
}

struct Cache {
    accounts: LruCache<Address, Option<CachedAccount>>,
    storage: LruCache<(Address, U256), U256>,
    code: LruCache<B256, Vec<u8>>,
}

impl Cache {
    fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity).expect("capacity must be > 0");
        Self {
            accounts: LruCache::new(cap),
            storage: LruCache::new(cap),
            code: LruCache::new(cap),
        }
    }
}

// ── API response types ─────────────────────────────────────────────

#[derive(Deserialize)]
struct AccountResponse {
    nonce: u64,
    balance: String,
    code_hash: String,
}

#[derive(Deserialize)]
struct StorageResponse {
    value: String,
}

#[derive(Deserialize)]
struct CodeResponse {
    code: String,
}

#[derive(Serialize)]
struct PrefetchRequestBody {
    to: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
}

#[derive(Deserialize)]
struct PrefetchResponseBody {
    result: String,
    success: bool,
    state_slice: StateSlice,
}

#[derive(Deserialize)]
struct StateSlice {
    accounts: std::collections::BTreeMap<String, SliceAccount>,
    storage: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    code: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize)]
struct SliceAccount {
    nonce: u64,
    balance: String,
    code_hash: String,
}

// ── Public types ───────────────────────────────────────────────────

/// Result of a prefetch call.
pub struct PrefetchResult {
    /// Raw output bytes from the EVM execution.
    pub result: Bytes,
    /// Whether the EVM call succeeded.
    pub success: bool,
}

// ── RemoteDB ───────────────────────────────────────────────────────

const DEFAULT_CACHE_CAPACITY: usize = 10_000;

/// A revm `DatabaseRef` that fetches EVM state over HTTP from a state API server.
///
/// Includes an LRU cache so repeated reads for the same key are served locally.
/// Call [`prefetch()`](Self::prefetch) before execution to preload all accessed
/// state in one round-trip.
pub struct RemoteDB {
    client: ureq::Agent,
    base_url: String,
    cache: Mutex<Cache>,
}

impl RemoteDB {
    /// Create a new `RemoteDB` pointing at the given base URL (e.g. `http://localhost:3000`).
    pub fn new(base_url: &str) -> Self {
        Self::with_capacity(base_url, DEFAULT_CACHE_CAPACITY)
    }

    /// Create a new `RemoteDB` with a custom LRU cache capacity per cache tier.
    pub fn with_capacity(base_url: &str, capacity: usize) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build();
        Self {
            client: config.into(),
            base_url: base_url.trim_end_matches('/').to_string(),
            cache: Mutex::new(Cache::new(capacity)),
        }
    }

    /// Speculatively execute a call on the server and preload all accessed state into the cache.
    pub fn prefetch(
        &self,
        to: Address,
        calldata: &[u8],
        from: Option<Address>,
        value: Option<U256>,
    ) -> Result<PrefetchResult, Error> {
        let body = PrefetchRequestBody {
            to: format!("{:#x}", to),
            data: Some(format!("0x{}", hex::encode(calldata))),
            from: from.map(|a| format!("{:#x}", a)),
            value: value.map(|v| format!("{:#x}", v)),
        };

        let url = format!("{}/v1/prefetch", self.base_url);
        let response = self.client.post(&url).send_json(&body)?;
        if response.status().as_u16() != 200 {
            return Err(Error::Server(response.status().as_u16()));
        }

        let resp: PrefetchResponseBody = response.into_body().read_json()?;

        // Populate cache from state_slice
        let mut cache = self.cache.lock().unwrap();

        for (addr_hex, acct) in &resp.state_slice.accounts {
            if let Ok(addr) = addr_hex.parse::<Address>() {
                let balance = parse_u256_hex(&acct.balance)?;
                let code_hash = parse_b256_hex(&acct.code_hash)?;
                cache.accounts.put(
                    addr,
                    Some(CachedAccount {
                        nonce: acct.nonce,
                        balance,
                        code_hash,
                    }),
                );
            }
        }

        for (addr_hex, slots) in &resp.state_slice.storage {
            if let Ok(addr) = addr_hex.parse::<Address>() {
                for (slot_hex, val_hex) in slots {
                    if let Ok(slot) = parse_b256_hex(slot_hex) {
                        let val = parse_u256_hex(val_hex)?;
                        cache.storage.put((addr, U256::from_be_bytes(slot.0)), val);
                    }
                }
            }
        }

        for (hash_hex, code_hex) in &resp.state_slice.code {
            if let Ok(hash) = parse_b256_hex(hash_hex) {
                let code_bytes = decode_hex_bytes(code_hex)?;
                cache.code.put(hash, code_bytes);
            }
        }

        let result_bytes = decode_hex_bytes(&resp.result)?;
        Ok(PrefetchResult {
            result: Bytes::from(result_bytes),
            success: resp.success,
        })
    }

    // ── Internal HTTP helpers ──────────────────────────────────────

    fn fetch_account(&self, address: &Address) -> Result<Option<CachedAccount>, Error> {
        let url = format!("{}/v1/account/{:#x}", self.base_url, address);
        let response = self.client.get(&url).call()?;

        match response.status().as_u16() {
            200 => {
                let resp: AccountResponse = response.into_body().read_json()?;
                let balance = parse_u256_hex(&resp.balance)?;
                let code_hash = parse_b256_hex(&resp.code_hash)?;
                let cached = CachedAccount {
                    nonce: resp.nonce,
                    balance,
                    code_hash,
                };
                // Proactively fetch code if the account has a contract
                if code_hash != B256::ZERO
                    && code_hash != alloy_primitives::KECCAK256_EMPTY
                {
                    self.fetch_code_for_address(address, &code_hash)?;
                }
                Ok(Some(cached))
            }
            404 => Ok(None),
            status => Err(Error::Server(status)),
        }
    }

    fn fetch_code_for_address(
        &self,
        address: &Address,
        code_hash: &B256,
    ) -> Result<(), Error> {
        // Check if already cached
        {
            let cache = self.cache.lock().unwrap();
            if cache.code.contains(code_hash) {
                return Ok(());
            }
        }

        let url = format!("{}/v1/code/{:#x}", self.base_url, address);
        let response = self.client.get(&url).call()?;

        match response.status().as_u16() {
            200 => {
                let resp: CodeResponse = response.into_body().read_json()?;
                let code_bytes = decode_hex_bytes(&resp.code)?;
                let mut cache = self.cache.lock().unwrap();
                cache.code.put(*code_hash, code_bytes);
                Ok(())
            }
            404 => Ok(()), // no code found, that's fine
            status => Err(Error::Server(status)),
        }
    }

    fn fetch_storage(&self, address: &Address, index: &U256) -> Result<U256, Error> {
        let slot = B256::from(*index);
        let url = format!("{}/v1/storage/{:#x}/{:#x}", self.base_url, address, slot);
        let response = self.client.get(&url).call()?;

        match response.status().as_u16() {
            200 => {
                let resp: StorageResponse = response.into_body().read_json()?;
                parse_u256_hex(&resp.value)
            }
            404 => Ok(U256::ZERO),
            status => Err(Error::Server(status)),
        }
    }
}

impl DatabaseRef for RemoteDB {
    type Error = Error;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        // Check cache first
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(cached) = cache.accounts.get(&address) {
                return Ok(cached.clone().map(|c| AccountInfo {
                    nonce: c.nonce,
                    balance: c.balance,
                    code_hash: c.code_hash,
                    code: None,
                }));
            }
        }

        // Fetch from server
        let result = self.fetch_account(&address)?;

        // Cache the result (including None for missing accounts)
        let mut cache = self.cache.lock().unwrap();
        cache.accounts.put(address, result.clone());

        Ok(result.map(|c| AccountInfo {
            nonce: c.nonce,
            balance: c.balance,
            code_hash: c.code_hash,
            code: None,
        }))
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let mut cache = self.cache.lock().unwrap();
        if let Some(bytes) = cache.code.get(&code_hash) {
            return Ok(Bytecode::new_raw(Bytes::copy_from_slice(bytes)));
        }
        // Code should have been pre-fetched by basic_ref or prefetch.
        // Return empty bytecode if not found.
        Ok(Bytecode::default())
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        // Check cache first
        {
            let mut cache = self.cache.lock().unwrap();
            if let Some(&value) = cache.storage.get(&(address, index)) {
                return Ok(value);
            }
        }

        // Fetch from server
        let value = self.fetch_storage(&address, &index)?;

        // Cache the result
        let mut cache = self.cache.lock().unwrap();
        cache.storage.put((address, index), value);

        Ok(value)
    }

    fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

// ── StateClient — higher-level call API ───────────────────────────

/// Trust mode for the [`StateClient`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustMode {
    /// Use the server's execution result directly without local verification.
    TrustServer,
    /// Re-execute locally with revm and verify the result matches the server.
    Verify,
}

/// Result of a [`StateClient::call`] invocation.
pub struct CallResult {
    /// Raw output bytes from the EVM execution.
    pub output: Bytes,
    /// Whether the EVM call succeeded.
    pub success: bool,
    /// `Some(true)` if local re-execution matched the server result,
    /// `Some(false)` if it diverged, `None` in `TrustServer` mode.
    pub verified: Option<bool>,
}

/// Higher-level client that combines [`RemoteDB`] prefetch with optional
/// local revm re-execution for trustless verification.
pub struct StateClient {
    db: RemoteDB,
    spec_id: revm::primitives::hardfork::SpecId,
    trust_mode: TrustMode,
}

impl StateClient {
    /// Create a new `StateClient`.
    pub fn new(
        base_url: &str,
        spec_id: revm::primitives::hardfork::SpecId,
        trust_mode: TrustMode,
    ) -> Self {
        Self {
            db: RemoteDB::new(base_url),
            spec_id,
            trust_mode,
        }
    }

    /// Access the underlying `RemoteDB`.
    pub fn remote_db(&self) -> &RemoteDB {
        &self.db
    }

    /// Execute an EVM call via the server's prefetch endpoint.
    ///
    /// In `TrustServer` mode, the server result is returned directly.
    /// In `Verify` mode, the call is re-executed locally with revm using
    /// the state cached from the prefetch response, and the results are compared.
    pub fn call(
        &self,
        to: Address,
        calldata: &[u8],
        from: Option<Address>,
        value: Option<U256>,
    ) -> Result<CallResult, Error> {
        let prefetch = self.db.prefetch(to, calldata, from, value)?;

        match self.trust_mode {
            TrustMode::TrustServer => Ok(CallResult {
                output: prefetch.result,
                success: prefetch.success,
                verified: None,
            }),
            TrustMode::Verify => {
                let local = self.execute_locally(to, calldata, from, value)?;
                let matches =
                    local.result == prefetch.result && local.success == prefetch.success;
                Ok(CallResult {
                    output: local.result,
                    success: local.success,
                    verified: Some(matches),
                })
            }
        }
    }

    fn execute_locally(
        &self,
        to: Address,
        calldata: &[u8],
        from: Option<Address>,
        value: Option<U256>,
    ) -> Result<PrefetchResult, Error> {
        use revm::database_interface::WrapDatabaseRef;
        use revm::{Context, ExecuteEvm, MainBuilder};

        let mut wrapped = WrapDatabaseRef(&self.db);
        let ctx: revm::handler::MainnetContext<&mut WrapDatabaseRef<&RemoteDB>> =
            Context::new(&mut wrapped, self.spec_id);
        let mut evm = ctx.build_mainnet();

        let tx_env = revm::context::TxEnv {
            caller: from.unwrap_or_default(),
            kind: revm::primitives::TxKind::Call(to),
            data: Bytes::copy_from_slice(calldata),
            value: value.unwrap_or_default(),
            gas_limit: 30_000_000,
            gas_price: 0,
            ..Default::default()
        };

        let result = evm
            .transact(tx_env)
            .map_err(|e| Error::Evm(format!("{e:?}")))?;

        use revm::context_interface::result::{ExecutionResult, Output};

        let output = match &result.result {
            ExecutionResult::Success { output, .. } => match output {
                Output::Call(bytes) => bytes.clone(),
                Output::Create(bytes, _) => bytes.clone(),
            },
            ExecutionResult::Revert { output, .. } => output.clone(),
            ExecutionResult::Halt { .. } => Bytes::new(),
        };

        Ok(PrefetchResult {
            result: output,
            success: result.result.is_success(),
        })
    }
}

// ── Hex parsing helpers ────────────────────────────────────────────

fn parse_u256_hex(s: &str) -> Result<U256, Error> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    U256::from_str_radix(s, 16)
        .map_err(|e| Error::InvalidResponse(format!("invalid U256: {e}")))
}

fn parse_b256_hex(s: &str) -> Result<B256, Error> {
    s.parse::<B256>()
        .map_err(|e| Error::InvalidResponse(format!("invalid B256: {e}")))
}

fn decode_hex_bytes(s: &str) -> Result<Vec<u8>, Error> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| Error::InvalidResponse(format!("invalid hex: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use evm_state_api::{AppState, build_router};
    use evm_state_chain_spec::{ChainSpec, HardforkActivation, HardforkCondition};
    use evm_state_common::AccountInfo as CommonAccountInfo;
    use evm_state_db::StateDb;
    use revm::database_interface::DatabaseRef;
    use revm::primitives::hardfork::SpecId;
    use std::sync::Arc;

    /// Start a real API server on a random port and return the base URL.
    async fn start_server(state: Arc<AppState>) -> String {
        let router = build_router(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    fn test_state() -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path()).unwrap();
        let state = Arc::new(AppState {
            db,
            chain_spec: ChainSpec {
                chain_id: 1,
                name: "test",
                hardforks: vec![HardforkActivation {
                    condition: HardforkCondition::Block(0),
                    spec_id: SpecId::SHANGHAI,
                }],
                requires_state_diffs: false,
            },
        });
        (dir, state)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_account() {
        let (_dir, state) = test_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        state
            .db
            .set_account(
                &addr,
                &CommonAccountInfo {
                    nonce: 42,
                    balance: U256::from(1_000_000u64),
                    code_hash: B256::ZERO,
                },
            )
            .unwrap();

        let base_url = start_server(state).await;
        let remote = RemoteDB::new(&base_url);

        let info = remote.basic_ref(addr).unwrap().unwrap();
        assert_eq!(info.nonce, 42);
        assert_eq!(info.balance, U256::from(1_000_000u64));
        assert_eq!(info.code_hash, B256::ZERO);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_storage() {
        let (_dir, state) = test_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let slot = B256::ZERO;
        state
            .db
            .set_storage(&addr, &slot, &U256::from(0xBEEFu64))
            .unwrap();

        let base_url = start_server(state).await;
        let remote = RemoteDB::new(&base_url);

        let value = remote.storage_ref(addr, U256::ZERO).unwrap();
        assert_eq!(value, U256::from(0xBEEFu64));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fetch_code() {
        let (_dir, state) = test_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let bytecode = vec![0x60, 0x42, 0x60, 0x00, 0xF3];
        let code_hash = alloy_primitives::keccak256(&bytecode);
        state
            .db
            .set_account(
                &addr,
                &CommonAccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: code_hash.into(),
                },
            )
            .unwrap();
        state.db.set_code(&code_hash.into(), &bytecode).unwrap();

        let base_url = start_server(state).await;
        let remote = RemoteDB::new(&base_url);

        // basic_ref should proactively fetch code
        let info = remote.basic_ref(addr).unwrap().unwrap();
        assert_eq!(info.code_hash, B256::from(code_hash));

        // code_by_hash_ref should find it in cache
        let code = remote.code_by_hash_ref(code_hash.into()).unwrap();
        assert_eq!(code.original_bytes(), Bytes::from(bytecode));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cache_hit() {
        let (_dir, state) = test_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        state
            .db
            .set_account(
                &addr,
                &CommonAccountInfo {
                    nonce: 7,
                    balance: U256::ZERO,
                    code_hash: B256::ZERO,
                },
            )
            .unwrap();

        let base_url = start_server(state).await;
        let remote = RemoteDB::new(&base_url);

        // First call populates cache
        let info1 = remote.basic_ref(addr).unwrap().unwrap();
        assert_eq!(info1.nonce, 7);

        // Second call should hit cache (same result)
        let info2 = remote.basic_ref(addr).unwrap().unwrap();
        assert_eq!(info2.nonce, 7);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn not_found_returns_defaults() {
        let (_dir, state) = test_state();

        let base_url = start_server(state).await;
        let remote = RemoteDB::new(&base_url);

        let addr = "0x0000000000000000000000000000000000000001"
            .parse::<Address>()
            .unwrap();

        // Missing account → None
        let info = remote.basic_ref(addr).unwrap();
        assert!(info.is_none());

        // Missing storage → U256::ZERO
        let value = remote.storage_ref(addr, U256::ZERO).unwrap();
        assert_eq!(value, U256::ZERO);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn prefetch_populates_cache() {
        let (_dir, state) = test_state();
        state.db.set_head_block(100).unwrap();

        // Contract that reads slot 0 and returns it
        let bytecode = vec![
            0x60, 0x00, // PUSH1 0x00
            0x54, // SLOAD
            0x60, 0x00, // PUSH1 0x00
            0x52, // MSTORE
            0x60, 0x20, // PUSH1 0x20
            0x60, 0x00, // PUSH1 0x00
            0xF3, // RETURN
        ];
        let code_hash = alloy_primitives::keccak256(&bytecode);
        let contract = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();

        state
            .db
            .set_account(
                &contract,
                &CommonAccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: code_hash.into(),
                },
            )
            .unwrap();
        state.db.set_code(&code_hash.into(), &bytecode).unwrap();
        state
            .db
            .set_storage(&contract, &B256::ZERO, &U256::from(0xCAFEu64))
            .unwrap();

        let base_url = start_server(state).await;
        let remote = RemoteDB::new(&base_url);

        // Prefetch populates cache
        let prefetch_result = remote.prefetch(contract, &[], None, None).unwrap();
        assert!(prefetch_result.success);
        assert!(prefetch_result.result.len() > 0);

        // Now all values should come from cache
        let info = remote.basic_ref(contract).unwrap().unwrap();
        assert_eq!(info.code_hash, B256::from(code_hash));

        let code = remote.code_by_hash_ref(code_hash.into()).unwrap();
        assert_eq!(code.original_bytes(), Bytes::from(bytecode));

        let storage = remote.storage_ref(contract, U256::ZERO).unwrap();
        assert_eq!(storage, U256::from(0xCAFEu64));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn block_hash_returns_zero() {
        let remote = RemoteDB::new("http://unused:9999");
        assert_eq!(remote.block_hash_ref(12345).unwrap(), B256::ZERO);
    }

    /// Helper: set up a contract that reads slot 0 and returns it.
    fn deploy_return_slot0(state: &Arc<AppState>) -> Address {
        // SLOAD(0) → MSTORE(0) → RETURN(0, 32)
        let bytecode = vec![
            0x60, 0x00, // PUSH1 0x00
            0x54, // SLOAD
            0x60, 0x00, // PUSH1 0x00
            0x52, // MSTORE
            0x60, 0x20, // PUSH1 0x20
            0x60, 0x00, // PUSH1 0x00
            0xF3, // RETURN
        ];
        let code_hash = alloy_primitives::keccak256(&bytecode);
        let contract = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();

        state
            .db
            .set_account(
                &contract,
                &CommonAccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: code_hash.into(),
                },
            )
            .unwrap();
        state.db.set_code(&code_hash.into(), &bytecode).unwrap();
        state
            .db
            .set_storage(&contract, &B256::ZERO, &U256::from(0xCAFEu64))
            .unwrap();
        state.db.set_head_block(100).unwrap();
        contract
    }

    /// Helper: set up a contract that always reverts.
    fn deploy_reverting(state: &Arc<AppState>) -> Address {
        // PUSH1 0x00, PUSH1 0x00, REVERT
        let bytecode = vec![
            0x60, 0x00, // PUSH1 0x00
            0x60, 0x00, // PUSH1 0x00
            0xFD, // REVERT
        ];
        let code_hash = alloy_primitives::keccak256(&bytecode);
        let contract = "0x0000000000000000000000000000000000BEEF01"
            .parse::<Address>()
            .unwrap();

        state
            .db
            .set_account(
                &contract,
                &CommonAccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: code_hash.into(),
                },
            )
            .unwrap();
        state.db.set_code(&code_hash.into(), &bytecode).unwrap();
        state.db.set_head_block(100).unwrap();
        contract
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_trust_server_returns_result() {
        let (_dir, state) = test_state();
        let contract = deploy_return_slot0(&state);

        let base_url = start_server(state).await;
        let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::TrustServer);

        let result = client.call(contract, &[], None, None).unwrap();
        assert!(result.success);
        assert!(result.verified.is_none());
        // Should return 32 bytes with 0xCAFE
        assert_eq!(result.output.len(), 32);
        let val = U256::from_be_slice(&result.output);
        assert_eq!(val, U256::from(0xCAFEu64));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_verify_mode_matches() {
        let (_dir, state) = test_state();
        let contract = deploy_return_slot0(&state);

        let base_url = start_server(state).await;
        let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::Verify);

        let result = client.call(contract, &[], None, None).unwrap();
        assert!(result.success);
        assert_eq!(result.verified, Some(true));
        let val = U256::from_be_slice(&result.output);
        assert_eq!(val, U256::from(0xCAFEu64));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_verify_mode_populates_cache() {
        let (_dir, state) = test_state();
        let contract = deploy_return_slot0(&state);

        let base_url = start_server(state).await;
        let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::Verify);

        let _ = client.call(contract, &[], None, None).unwrap();

        // After call(), the cache should contain the contract account + storage
        let info = client
            .remote_db()
            .basic_ref(contract)
            .unwrap()
            .unwrap();
        assert_eq!(info.code_hash, B256::from(alloy_primitives::keccak256(&[
            0x60, 0x00, 0x54, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3,
        ])));

        let storage = client.remote_db().storage_ref(contract, U256::ZERO).unwrap();
        assert_eq!(storage, U256::from(0xCAFEu64));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_trust_server_reverted_tx() {
        let (_dir, state) = test_state();
        let contract = deploy_reverting(&state);

        let base_url = start_server(state).await;
        let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::TrustServer);

        let result = client.call(contract, &[], None, None).unwrap();
        assert!(!result.success);
        assert!(result.verified.is_none());
    }
}
