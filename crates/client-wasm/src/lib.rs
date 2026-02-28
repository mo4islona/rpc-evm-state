use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use alloy_primitives::{hex, Address, Bytes, B256, U256};
use revm::bytecode::Bytecode;
use revm::database_interface::{DBErrorMarker, DatabaseRef};
use revm::state::AccountInfo;
use serde::{Deserialize, Serialize};
use wasm_bindgen::prelude::*;

// ── Error type ─────────────────────────────────────────────────────

#[derive(Debug)]
struct WasmError(String);

impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for WasmError {}
impl DBErrorMarker for WasmError {}

impl From<WasmError> for JsValue {
    fn from(e: WasmError) -> JsValue {
        JsValue::from_str(&e.0)
    }
}

// ── API response types (mirrored from crates/client) ──────────────

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

// ── Cache (single-threaded, RefCell) ──────────────────────────────

#[derive(Clone)]
struct CachedAccount {
    nonce: u64,
    balance: U256,
    code_hash: B256,
}

struct Cache {
    accounts: HashMap<Address, Option<CachedAccount>>,
    storage: HashMap<(Address, U256), U256>,
    code: HashMap<B256, Vec<u8>>,
}

impl Cache {
    fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            storage: HashMap::new(),
            code: HashMap::new(),
        }
    }
}

// ── WasmRemoteDB — DatabaseRef backed by in-memory cache ─────────

struct WasmRemoteDB {
    cache: Rc<RefCell<Cache>>,
}

impl WasmRemoteDB {
    fn new(cache: Rc<RefCell<Cache>>) -> Self {
        Self { cache }
    }
}

impl DatabaseRef for WasmRemoteDB {
    type Error = WasmError;

    fn basic_ref(&self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        let cache = self.cache.borrow();
        match cache.accounts.get(&address) {
            Some(Some(cached)) => Ok(Some(AccountInfo {
                nonce: cached.nonce,
                balance: cached.balance,
                code_hash: cached.code_hash,
                code: cache.code.get(&cached.code_hash).map(|bytes| {
                    Bytecode::new_raw(Bytes::copy_from_slice(bytes))
                }),
            })),
            Some(None) => Ok(None),
            None => Ok(None),
        }
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let cache = self.cache.borrow();
        Ok(cache.storage.get(&(address, index)).copied().unwrap_or(U256::ZERO))
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let cache = self.cache.borrow();
        match cache.code.get(&code_hash) {
            Some(bytes) => Ok(Bytecode::new_raw(Bytes::copy_from_slice(bytes))),
            None => Ok(Bytecode::default()),
        }
    }

    fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

// ── Async HTTP via web-sys fetch ──────────────────────────────────

async fn fetch_post_json(url: &str, body: &str) -> Result<String, WasmError> {
    use web_sys::{Request, RequestInit, Response};

    let opts = RequestInit::new();
    opts.set_method("POST");

    let headers = web_sys::Headers::new()
        .map_err(|e| WasmError(format!("failed to create headers: {e:?}")))?;
    headers
        .set("Content-Type", "application/json")
        .map_err(|e| WasmError(format!("failed to set header: {e:?}")))?;
    opts.set_headers(&headers);
    opts.set_body(&JsValue::from_str(body));

    let request = Request::new_with_str_and_init(url, &opts)
        .map_err(|e| WasmError(format!("failed to create request: {e:?}")))?;

    // Use the global fetch function (works in browsers, workers, and Node.js 18+)
    let global = js_sys::global();
    let fetch_fn = js_sys::Reflect::get(&global, &JsValue::from_str("fetch"))
        .map_err(|e| WasmError(format!("no fetch function found: {e:?}")))?;
    let fetch_fn = fetch_fn
        .dyn_ref::<js_sys::Function>()
        .ok_or_else(|| WasmError("fetch is not a function".to_string()))?;
    let promise = fetch_fn
        .call1(&JsValue::NULL, &request)
        .map_err(|e| WasmError(format!("fetch call failed: {e:?}")))?;
    let resp_value = wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(promise))
        .await
        .map_err(|e| WasmError(format!("fetch failed: {e:?}")))?;

    let resp: Response = resp_value
        .dyn_into()
        .map_err(|_| WasmError("response is not a Response".to_string()))?;

    if !resp.ok() {
        return Err(WasmError(format!("HTTP {}", resp.status())));
    }

    let text_promise = resp
        .text()
        .map_err(|e| WasmError(format!("failed to read body: {e:?}")))?;
    let text_value = wasm_bindgen_futures::JsFuture::from(text_promise)
        .await
        .map_err(|e| WasmError(format!("failed to read body text: {e:?}")))?;

    text_value
        .as_string()
        .ok_or_else(|| WasmError("response body is not a string".to_string()))
}

// ── Hex parsing helpers ───────────────────────────────────────────

fn parse_u256_hex(s: &str) -> Result<U256, WasmError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    U256::from_str_radix(s, 16).map_err(|e| WasmError(format!("invalid U256: {e}")))
}

fn parse_b256_hex(s: &str) -> Result<B256, WasmError> {
    s.parse::<B256>()
        .map_err(|e| WasmError(format!("invalid B256: {e}")))
}

fn decode_hex_bytes(s: &str) -> Result<Vec<u8>, WasmError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s).map_err(|e| WasmError(format!("invalid hex: {e}")))
}

// ── Prefetch & cache populate ─────────────────────────────────────

struct PrefetchResult {
    result: Bytes,
    success: bool,
}

async fn do_prefetch(
    endpoint: &str,
    cache: &Rc<RefCell<Cache>>,
    to: &str,
    calldata: &str,
    from: Option<&str>,
    value: Option<&str>,
) -> Result<PrefetchResult, WasmError> {
    let body = PrefetchRequestBody {
        to: to.to_string(),
        data: if calldata.is_empty() {
            None
        } else {
            Some(calldata.to_string())
        },
        from: from.map(|s| s.to_string()),
        value: value.map(|s| s.to_string()),
    };

    let body_json =
        serde_json::to_string(&body).map_err(|e| WasmError(format!("serialize error: {e}")))?;

    let url = format!("{}/v1/prefetch", endpoint.trim_end_matches('/'));
    let response_text = fetch_post_json(&url, &body_json).await?;

    let resp: PrefetchResponseBody =
        serde_json::from_str(&response_text).map_err(|e| WasmError(format!("parse error: {e}")))?;

    // Populate cache from state_slice
    let mut cache = cache.borrow_mut();

    for (addr_hex, acct) in &resp.state_slice.accounts {
        if let Ok(addr) = addr_hex.parse::<Address>() {
            let balance = parse_u256_hex(&acct.balance)?;
            let code_hash = parse_b256_hex(&acct.code_hash)?;
            cache.accounts.insert(
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
                    cache
                        .storage
                        .insert((addr, U256::from_be_bytes(slot.0)), val);
                }
            }
        }
    }

    for (hash_hex, code_hex) in &resp.state_slice.code {
        if let Ok(hash) = parse_b256_hex(hash_hex) {
            let code_bytes = decode_hex_bytes(code_hex)?;
            cache.code.insert(hash, code_bytes);
        }
    }

    let result_bytes = decode_hex_bytes(&resp.result)?;
    Ok(PrefetchResult {
        result: Bytes::from(result_bytes),
        success: resp.success,
    })
}

// ── Local EVM execution ───────────────────────────────────────────

fn execute_locally(
    db: &WasmRemoteDB,
    to: Address,
    calldata: &[u8],
    from: Option<Address>,
    value: Option<U256>,
) -> Result<PrefetchResult, WasmError> {
    use revm::context_interface::result::{ExecutionResult, Output};
    use revm::database_interface::WrapDatabaseRef;
    use revm::primitives::hardfork::SpecId;
    use revm::{Context, ExecuteEvm, MainBuilder};

    let mut wrapped = WrapDatabaseRef(db);
    let ctx: revm::handler::MainnetContext<&mut WrapDatabaseRef<&WasmRemoteDB>> =
        Context::new(&mut wrapped, SpecId::CANCUN);
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
        .map_err(|e| WasmError(format!("EVM error: {e:?}")))?;

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

// ── Exported JS API ───────────────────────────────────────────────

/// WASM-compiled EVM state client.
///
/// Fetches state from a remote API server via the prefetch endpoint,
/// optionally re-executes locally with revm for verification.
#[wasm_bindgen]
pub struct WasmStateClient {
    endpoint: String,
    cache: Rc<RefCell<Cache>>,
    verify: bool,
}

#[wasm_bindgen]
impl WasmStateClient {
    /// Create a new client pointing at the given API endpoint.
    ///
    /// If `verify` is true, calls are re-executed locally with revm.
    #[wasm_bindgen(constructor)]
    pub fn new(endpoint: &str, verify: bool) -> Self {
        Self {
            endpoint: endpoint.trim_end_matches('/').to_string(),
            cache: Rc::new(RefCell::new(Cache::new())),
            verify,
        }
    }

    /// Execute an EVM call via the server's prefetch endpoint.
    ///
    /// Returns a JS object: `{ output: "0x...", success: boolean, verified: boolean | null }`
    pub async fn call(
        &self,
        to: &str,
        calldata: &str,
        from: Option<String>,
        value: Option<String>,
    ) -> Result<JsValue, JsValue> {
        let prefetch = do_prefetch(
            &self.endpoint,
            &self.cache,
            to,
            calldata,
            from.as_deref(),
            value.as_deref(),
        )
        .await?;

        if !self.verify {
            // TrustServer mode — return server result directly
            let obj = js_sys::Object::new();
            js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("output"),
                &JsValue::from_str(&format!("0x{}", hex::encode(&prefetch.result))),
            )?;
            js_sys::Reflect::set(
                &obj,
                &JsValue::from_str("success"),
                &JsValue::from_bool(prefetch.success),
            )?;
            js_sys::Reflect::set(&obj, &JsValue::from_str("verified"), &JsValue::NULL)?;
            return Ok(obj.into());
        }

        // Verify mode — re-execute locally
        let to_addr = to
            .parse::<Address>()
            .map_err(|e| JsValue::from_str(&format!("invalid to address: {e}")))?;

        let calldata_bytes = decode_hex_bytes(calldata)
            .map_err(|e| JsValue::from_str(&e.0))?;

        let from_addr = from
            .as_deref()
            .map(|s| s.parse::<Address>())
            .transpose()
            .map_err(|e| JsValue::from_str(&format!("invalid from address: {e}")))?;

        let value_u256 = value
            .as_deref()
            .map(|s| parse_u256_hex(s))
            .transpose()
            .map_err(|e| JsValue::from_str(&e.0))?;

        let db = WasmRemoteDB::new(Rc::clone(&self.cache));
        let local = execute_locally(&db, to_addr, &calldata_bytes, from_addr, value_u256)
            .map_err(|e| JsValue::from_str(&e.0))?;

        let matches = local.result == prefetch.result && local.success == prefetch.success;

        let obj = js_sys::Object::new();
        js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("output"),
            &JsValue::from_str(&format!("0x{}", hex::encode(&local.result))),
        )?;
        js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("success"),
            &JsValue::from_bool(local.success),
        )?;
        js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("verified"),
            &JsValue::from_bool(matches),
        )?;
        Ok(obj.into())
    }

    /// Speculatively execute a call on the server and preload state into the cache.
    ///
    /// Returns a JS object: `{ result: "0x...", success: boolean }`
    pub async fn prefetch(
        &self,
        to: &str,
        calldata: &str,
    ) -> Result<JsValue, JsValue> {
        let result = do_prefetch(&self.endpoint, &self.cache, to, calldata, None, None).await?;

        let obj = js_sys::Object::new();
        js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("result"),
            &JsValue::from_str(&format!("0x{}", hex::encode(&result.result))),
        )?;
        js_sys::Reflect::set(
            &obj,
            &JsValue::from_str("success"),
            &JsValue::from_bool(result.success),
        )?;
        Ok(obj.into())
    }
}

/// Create a new WASM state client.
///
/// This is a convenience factory for use from JavaScript:
/// ```js
/// import { createClient } from 'evm-state-client-wasm';
/// const client = createClient("http://localhost:3000", false);
/// ```
#[wasm_bindgen(js_name = createClient)]
pub fn create_client(endpoint: &str, verify: bool) -> WasmStateClient {
    WasmStateClient::new(endpoint, verify)
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_helpers() {
        let u = parse_u256_hex("0x1234").unwrap();
        assert_eq!(u, U256::from(0x1234u64));

        let b = parse_b256_hex(
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .unwrap();
        assert_eq!(b, B256::with_last_byte(1));

        let bytes = decode_hex_bytes("0xdeadbeef").unwrap();
        assert_eq!(bytes, vec![0xde, 0xad, 0xbe, 0xef]);
    }

    #[test]
    fn cache_database_ref_returns_defaults_on_miss() {
        let cache = Rc::new(RefCell::new(Cache::new()));
        let db = WasmRemoteDB::new(cache);

        let addr = Address::ZERO;
        assert!(db.basic_ref(addr).unwrap().is_none());
        assert_eq!(db.storage_ref(addr, U256::ZERO).unwrap(), U256::ZERO);
        assert_eq!(db.block_hash_ref(0).unwrap(), B256::ZERO);
    }

    #[test]
    fn cache_database_ref_reads_populated_data() {
        let cache = Rc::new(RefCell::new(Cache::new()));
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let code_hash = B256::with_last_byte(0x42);
        let code_bytes = vec![0x60, 0x00, 0xF3];

        {
            let mut c = cache.borrow_mut();
            c.accounts.insert(
                addr,
                Some(CachedAccount {
                    nonce: 7,
                    balance: U256::from(1_000_000u64),
                    code_hash,
                }),
            );
            c.storage
                .insert((addr, U256::ZERO), U256::from(0xBEEFu64));
            c.code.insert(code_hash, code_bytes.clone());
        }

        let db = WasmRemoteDB::new(cache);

        let info = db.basic_ref(addr).unwrap().unwrap();
        assert_eq!(info.nonce, 7);
        assert_eq!(info.balance, U256::from(1_000_000u64));
        assert_eq!(info.code_hash, code_hash);
        assert!(info.code.is_some());

        let storage = db.storage_ref(addr, U256::ZERO).unwrap();
        assert_eq!(storage, U256::from(0xBEEFu64));

        let code = db.code_by_hash_ref(code_hash).unwrap();
        assert_eq!(code.original_bytes(), Bytes::from(code_bytes));
    }

    #[test]
    fn local_execution_with_cached_state() {
        let cache = Rc::new(RefCell::new(Cache::new()));

        // Deploy a simple contract: PUSH1 0x42, PUSH1 0x00, MSTORE, PUSH1 0x20, PUSH1 0x00, RETURN
        let bytecode = vec![
            0x60, 0x42, // PUSH1 0x42
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

        {
            let mut c = cache.borrow_mut();
            c.accounts.insert(
                contract,
                Some(CachedAccount {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: code_hash.into(),
                }),
            );
            c.code.insert(code_hash.into(), bytecode);
        }

        let db = WasmRemoteDB::new(cache);
        let result = execute_locally(&db, contract, &[], None, None).unwrap();

        assert!(result.success);
        assert_eq!(result.result.len(), 32);
        // The contract returns 0x42 stored at memory offset 0
        assert_eq!(result.result[31], 0x42);
    }
}
