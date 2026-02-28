use std::collections::BTreeMap;
use std::sync::Arc;

use alloy_primitives::{hex, Address, Bytes, B256, U256};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use axum::{Json, Router};
use evm_state_chain_spec::ChainSpec;
use evm_state_db::StateDb;
use revm::context::TxEnv;
use revm::database_interface::WrapDatabaseRef;
use revm::primitives::TxKind;
use revm::{Context, InspectEvm, MainBuilder};
use serde::{Deserialize, Serialize};
use tower_http::cors::CorsLayer;

mod inspector;
use inspector::AccessListInspector;

// ── Shared state ────────────────────────────────────────────────────

/// Shared application state provided to all handlers.
pub struct AppState {
    pub db: StateDb,
    pub chain_spec: ChainSpec,
}

// ── Error handling ──────────────────────────────────────────────────

enum AppError {
    BadRequest(String),
    NotFound,
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found".into()),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl From<evm_state_db::Error> for AppError {
    fn from(e: evm_state_db::Error) -> Self {
        AppError::Internal(e.to_string())
    }
}

type AppResult<T> = Result<T, AppError>;

// ── Response types ──────────────────────────────────────────────────

#[derive(Serialize)]
struct HeadResponse {
    head_block: u64,
}

#[derive(Serialize)]
struct AccountResponse {
    nonce: u64,
    balance: String,
    code_hash: String,
}

#[derive(Serialize)]
struct StorageResponse {
    value: String,
}

#[derive(Serialize)]
struct CodeResponse {
    code: String,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn parse_address(s: &str) -> Result<Address, AppError> {
    s.parse::<Address>()
        .map_err(|_| AppError::BadRequest(format!("invalid address: {s}")))
}

fn parse_b256(s: &str) -> Result<B256, AppError> {
    s.parse::<B256>()
        .map_err(|_| AppError::BadRequest(format!("invalid B256: {s}")))
}

// ── Handlers ────────────────────────────────────────────────────────

async fn get_head(State(state): State<Arc<AppState>>) -> AppResult<impl IntoResponse> {
    let head = state.db.get_head_block()?.ok_or(AppError::NotFound)?;
    Ok(Json(HeadResponse { head_block: head }))
}

async fn get_account(
    State(state): State<Arc<AppState>>,
    Path(addr): Path<String>,
) -> AppResult<impl IntoResponse> {
    let address = parse_address(&addr)?;
    let info = state.db.get_account(&address)?.ok_or(AppError::NotFound)?;
    Ok(Json(AccountResponse {
        nonce: info.nonce,
        balance: format!("{:#x}", info.balance),
        code_hash: format!("{:#x}", info.code_hash),
    }))
}

async fn get_storage(
    State(state): State<Arc<AppState>>,
    Path((addr, slot)): Path<(String, String)>,
) -> AppResult<impl IntoResponse> {
    let address = parse_address(&addr)?;
    let slot = parse_b256(&slot)?;
    let value = state
        .db
        .get_storage(&address, &slot)?
        .ok_or(AppError::NotFound)?;
    Ok(Json(StorageResponse {
        value: format!("{:#x}", value),
    }))
}

async fn get_code(
    State(state): State<Arc<AppState>>,
    Path(addr): Path<String>,
) -> AppResult<impl IntoResponse> {
    let address = parse_address(&addr)?;
    let info = state.db.get_account(&address)?.ok_or(AppError::NotFound)?;
    let code = state
        .db
        .get_code(&info.code_hash)?
        .ok_or(AppError::NotFound)?;
    Ok(Json(CodeResponse {
        code: format!("0x{}", hex::encode(&code)),
    }))
}

// ── Batch endpoint ──────────────────────────────────────────────────

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BatchItem {
    Account { addr: String },
    Storage { addr: String, slot: String },
    Code { addr: String },
}

#[derive(Serialize)]
#[serde(untagged)]
enum BatchResult {
    Account(AccountResponse),
    Storage(StorageResponse),
    Code(CodeResponse),
    NotFound { error: &'static str },
    Error { error: String },
}

async fn post_batch(
    State(state): State<Arc<AppState>>,
    Json(items): Json<Vec<BatchItem>>,
) -> AppResult<impl IntoResponse> {
    let results: Vec<BatchResult> = items
        .iter()
        .map(|item| match item {
            BatchItem::Account { addr } => {
                let address = addr
                    .parse::<Address>()
                    .map_err(|_| format!("invalid address: {addr}"))?;
                match state.db.get_account(&address) {
                    Ok(Some(info)) => Ok(BatchResult::Account(AccountResponse {
                        nonce: info.nonce,
                        balance: format!("{:#x}", info.balance),
                        code_hash: format!("{:#x}", info.code_hash),
                    })),
                    Ok(None) => Ok(BatchResult::NotFound { error: "not found" }),
                    Err(e) => Err(e.to_string()),
                }
            }
            BatchItem::Storage { addr, slot } => {
                let address = addr
                    .parse::<Address>()
                    .map_err(|_| format!("invalid address: {addr}"))?;
                let slot = slot
                    .parse::<B256>()
                    .map_err(|_| format!("invalid slot: {slot}"))?;
                match state.db.get_storage(&address, &slot) {
                    Ok(Some(value)) => Ok(BatchResult::Storage(StorageResponse {
                        value: format!("{:#x}", value),
                    })),
                    Ok(None) => Ok(BatchResult::NotFound { error: "not found" }),
                    Err(e) => Err(e.to_string()),
                }
            }
            BatchItem::Code { addr } => {
                let address = addr
                    .parse::<Address>()
                    .map_err(|_| format!("invalid address: {addr}"))?;
                let info = match state.db.get_account(&address) {
                    Ok(Some(info)) => info,
                    Ok(None) => return Ok(BatchResult::NotFound { error: "not found" }),
                    Err(e) => return Err(e.to_string()),
                };
                match state.db.get_code(&info.code_hash) {
                    Ok(Some(code)) => Ok(BatchResult::Code(CodeResponse {
                        code: format!("0x{}", hex::encode(&code)),
                    })),
                    Ok(None) => Ok(BatchResult::NotFound { error: "not found" }),
                    Err(e) => Err(e.to_string()),
                }
            }
        })
        .map(|r| match r {
            Ok(result) => result,
            Err(msg) => BatchResult::Error { error: msg },
        })
        .collect();

    Ok(Json(results))
}

// ── Prefetch endpoint ───────────────────────────────────────────────

#[derive(Deserialize)]
struct PrefetchRequest {
    to: String,
    #[serde(default)]
    data: Option<String>,
    from: Option<String>,
    value: Option<String>,
}

#[derive(Serialize)]
struct AccessListEntry {
    address: String,
    slots: Vec<String>,
}

#[derive(Serialize)]
struct PrefetchAccountInfo {
    nonce: u64,
    balance: String,
    code_hash: String,
}

#[derive(Serialize)]
struct PrefetchStateSlice {
    accounts: BTreeMap<String, PrefetchAccountInfo>,
    storage: BTreeMap<String, BTreeMap<String, String>>,
    code: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct PrefetchResponse {
    result: String,
    success: bool,
    access_list: Vec<AccessListEntry>,
    state_slice: PrefetchStateSlice,
}

fn decode_hex_data(s: &str) -> Result<Bytes, AppError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    hex::decode(s)
        .map(Bytes::from)
        .map_err(|e| AppError::BadRequest(format!("invalid hex data: {e}")))
}

fn parse_u256(s: &str) -> Result<U256, AppError> {
    let s = s.strip_prefix("0x").unwrap_or(s);
    U256::from_str_radix(s, 16).map_err(|e| AppError::BadRequest(format!("invalid value: {e}")))
}

async fn post_prefetch(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PrefetchRequest>,
) -> AppResult<impl IntoResponse> {
    let to = parse_address(&req.to)?;
    let from = req
        .from
        .as_deref()
        .map(parse_address)
        .transpose()?
        .unwrap_or(Address::ZERO);
    let calldata = req
        .data
        .as_deref()
        .map(decode_hex_data)
        .transpose()?
        .unwrap_or_default();
    let value = req
        .value
        .as_deref()
        .map(parse_u256)
        .transpose()?
        .unwrap_or(U256::ZERO);

    let head_block = state.db.get_head_block()?.unwrap_or(0);
    let spec_id = state.chain_spec.spec_at(head_block, 0);

    // Use WrapDatabaseRef since we only have &StateDb (shared via Arc)
    let db_ref = WrapDatabaseRef(&state.db);
    let inspector = AccessListInspector::new();

    let ctx: revm::handler::MainnetContext<WrapDatabaseRef<&StateDb>> =
        Context::new(db_ref, spec_id);
    let ctx = ctx
        .modify_block_chained(|b: &mut revm::context::BlockEnv| {
            b.number = head_block;
            b.gas_limit = 30_000_000;
            b.basefee = 0;
            b.prevrandao = Some(B256::ZERO);
        })
        .modify_tx_chained(|t: &mut TxEnv| {
            t.caller = from;
            t.gas_limit = 30_000_000;
            t.kind = TxKind::Call(to);
            t.data = calldata;
            t.value = value;
            t.gas_price = 0;
        });

    let mut evm = ctx.build_mainnet_with_inspector(inspector);
    let result_and_state = evm
        .inspect_replay()
        .map_err(|e| AppError::Internal(format!("EVM error: {e:?}")))?;

    let output = result_and_state
        .result
        .output()
        .map(|o| format!("0x{}", hex::encode(o)))
        .unwrap_or_else(|| "0x".into());
    let success = result_and_state.result.is_success();

    // Get recorded accesses from the inspector
    let mut accesses = evm.inspector.into_accesses();
    // Ensure caller and target are included (accessed at tx level, not opcode level)
    accesses.entry(from).or_default();
    accesses.entry(to).or_default();

    // Build access_list and state_slice
    let mut access_list = Vec::new();
    let mut slice_accounts = BTreeMap::new();
    let mut slice_storage = BTreeMap::new();
    let mut slice_code = BTreeMap::new();

    for (address, slots) in &accesses {
        access_list.push(AccessListEntry {
            address: format!("{:#x}", address),
            slots: slots.iter().map(|s| format!("{:#x}", s)).collect(),
        });

        if let Ok(Some(info)) = state.db.get_account(address) {
            slice_accounts.insert(
                format!("{:#x}", address),
                PrefetchAccountInfo {
                    nonce: info.nonce,
                    balance: format!("{:#x}", info.balance),
                    code_hash: format!("{:#x}", info.code_hash),
                },
            );

            if info.code_hash != B256::ZERO
                && info.code_hash != alloy_primitives::KECCAK256_EMPTY
            {
                if let Ok(Some(code)) = state.db.get_code(&info.code_hash) {
                    slice_code.insert(
                        format!("{:#x}", info.code_hash),
                        format!("0x{}", hex::encode(&code)),
                    );
                }
            }
        }

        if !slots.is_empty() {
            let mut slot_values = BTreeMap::new();
            for slot in slots {
                let val = state.db.get_storage(address, slot)?.unwrap_or(U256::ZERO);
                slot_values.insert(format!("{:#x}", slot), format!("{:#x}", val));
            }
            slice_storage.insert(format!("{:#x}", address), slot_values);
        }
    }

    Ok(Json(PrefetchResponse {
        result: output,
        success,
        access_list,
        state_slice: PrefetchStateSlice {
            accounts: slice_accounts,
            storage: slice_storage,
            code: slice_code,
        },
    }))
}

// ── WebSocket endpoint ──────────────────────────────────────────────

#[derive(Deserialize)]
struct WsRequest {
    id: Option<serde_json::Value>,
    #[serde(flatten)]
    query: WsQuery,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WsQuery {
    Account { addr: String },
    Storage { addr: String, slot: String },
    Code { addr: String },
    Head,
}

#[derive(Serialize)]
struct WsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
    #[serde(flatten)]
    payload: WsPayload,
}

#[derive(Serialize)]
#[serde(untagged)]
enum WsPayload {
    Result { result: serde_json::Value },
    Error { error: String },
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(text) => {
                let response = process_ws_message(&text, &state);
                let json = serde_json::to_string(&response).unwrap();
                if socket.send(Message::Text(json.into())).await.is_err() {
                    return;
                }
            }
            Message::Close(_) => return,
            _ => {}
        }
    }
}

fn process_ws_message(text: &str, state: &AppState) -> WsResponse {
    let req: WsRequest = match serde_json::from_str(text) {
        Ok(r) => r,
        Err(e) => {
            return WsResponse {
                id: None,
                payload: WsPayload::Error {
                    error: format!("invalid request: {e}"),
                },
            };
        }
    };

    let id = req.id;
    let payload = match req.query {
        WsQuery::Head => match state.db.get_head_block() {
            Ok(Some(n)) => WsPayload::Result {
                result: serde_json::json!({ "head_block": n }),
            },
            Ok(None) => WsPayload::Error {
                error: "not found".into(),
            },
            Err(e) => WsPayload::Error {
                error: e.to_string(),
            },
        },
        WsQuery::Account { addr } => match addr.parse::<Address>() {
            Err(_) => WsPayload::Error {
                error: format!("invalid address: {addr}"),
            },
            Ok(address) => match state.db.get_account(&address) {
                Ok(Some(info)) => WsPayload::Result {
                    result: serde_json::json!({
                        "nonce": info.nonce,
                        "balance": format!("{:#x}", info.balance),
                        "code_hash": format!("{:#x}", info.code_hash),
                    }),
                },
                Ok(None) => WsPayload::Error {
                    error: "not found".into(),
                },
                Err(e) => WsPayload::Error {
                    error: e.to_string(),
                },
            },
        },
        WsQuery::Storage { addr, slot } => {
            let address = match addr.parse::<Address>() {
                Ok(a) => a,
                Err(_) => {
                    return WsResponse {
                        id,
                        payload: WsPayload::Error {
                            error: format!("invalid address: {addr}"),
                        },
                    };
                }
            };
            let slot = match slot.parse::<B256>() {
                Ok(s) => s,
                Err(_) => {
                    return WsResponse {
                        id,
                        payload: WsPayload::Error {
                            error: format!("invalid slot: {slot}"),
                        },
                    };
                }
            };
            match state.db.get_storage(&address, &slot) {
                Ok(Some(value)) => WsPayload::Result {
                    result: serde_json::json!({ "value": format!("{:#x}", value) }),
                },
                Ok(None) => WsPayload::Error {
                    error: "not found".into(),
                },
                Err(e) => WsPayload::Error {
                    error: e.to_string(),
                },
            }
        }
        WsQuery::Code { addr } => match addr.parse::<Address>() {
            Err(_) => WsPayload::Error {
                error: format!("invalid address: {addr}"),
            },
            Ok(address) => match state.db.get_account(&address) {
                Ok(Some(info)) => match state.db.get_code(&info.code_hash) {
                    Ok(Some(code)) => WsPayload::Result {
                        result: serde_json::json!({ "code": format!("0x{}", hex::encode(&code)) }),
                    },
                    Ok(None) => WsPayload::Error {
                        error: "not found".into(),
                    },
                    Err(e) => WsPayload::Error {
                        error: e.to_string(),
                    },
                },
                Ok(None) => WsPayload::Error {
                    error: "not found".into(),
                },
                Err(e) => WsPayload::Error {
                    error: e.to_string(),
                },
            },
        },
    };

    WsResponse { id, payload }
}

// ── Router ──────────────────────────────────────────────────────────

/// Build the axum [`Router`] with all v1 endpoints.
///
/// The caller provides shared [`AppState`] wrapped in an [`Arc`].
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/v1/head", get(get_head))
        .route("/v1/account/{addr}", get(get_account))
        .route("/v1/storage/{addr}/{slot}", get(get_storage))
        .route("/v1/code/{addr}", get(get_code))
        .route("/v1/batch", post(post_batch))
        .route("/v1/prefetch", post(post_prefetch))
        .route("/v1/stream", any(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use evm_state_common::AccountInfo;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn tmp_state() -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path()).unwrap();
        let state = Arc::new(AppState {
            db,
            chain_spec: ChainSpec {
                chain_id: 1,
                name: "test",
                hardforks: vec![evm_state_chain_spec::HardforkActivation {
                    condition: evm_state_chain_spec::HardforkCondition::Block(0),
                    spec_id: revm::primitives::hardfork::SpecId::SHANGHAI,
                }],
                requires_state_diffs: false,
            },
        });
        (dir, state)
    }

    async fn request(router: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let req = axum::http::Request::builder()
            .uri(uri)
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    async fn post_json(
        router: &Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        (status, json)
    }

    // ── /v1/head ────────────────────────────────────────────────────

    #[tokio::test]
    async fn head_block_returns_value() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(65_000_000).unwrap();
        let router = build_router(state);

        let (status, json) = request(&router, "/v1/head").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["head_block"], 65_000_000);
    }

    #[tokio::test]
    async fn head_block_not_set() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let (status, _) = request(&router, "/v1/head").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ── /v1/account/:addr ───────────────────────────────────────────

    #[tokio::test]
    async fn account_found() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let info = AccountInfo {
            nonce: 42,
            balance: U256::from(1_000_000u64),
            code_hash: B256::ZERO,
        };
        state.db.set_account(&addr, &info).unwrap();
        let router = build_router(state);

        let (status, json) = request(
            &router,
            "/v1/account/0x0000000000000000000000000000000000C0FFEE",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["nonce"], 42);
        assert!(json["balance"].as_str().unwrap().starts_with("0x"));
        assert!(json["code_hash"].as_str().unwrap().starts_with("0x"));
    }

    #[tokio::test]
    async fn account_not_found() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let (status, _) = request(
            &router,
            "/v1/account/0x0000000000000000000000000000000000000001",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn account_invalid_address() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let (status, json) = request(&router, "/v1/account/notanaddr").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid address"));
    }

    // ── /v1/storage/:addr/:slot ─────────────────────────────────────

    #[tokio::test]
    async fn storage_found() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let slot = B256::ZERO;
        let value = U256::from(0x42u64);
        state.db.set_storage(&addr, &slot, &value).unwrap();
        let router = build_router(state);

        let (status, json) = request(
            &router,
            &format!(
                "/v1/storage/0x0000000000000000000000000000000000C0FFEE/0x{:064x}",
                0u64
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["value"].as_str().unwrap(), "0x42");
    }

    #[tokio::test]
    async fn storage_not_found() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let (status, _) = request(
            &router,
            &format!(
                "/v1/storage/0x0000000000000000000000000000000000000001/0x{:064x}",
                0u64
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn storage_invalid_params() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let (status, json) = request(&router, "/v1/storage/badaddr/badslot").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid"));
    }

    // ── /v1/code/:addr ──────────────────────────────────────────────

    #[tokio::test]
    async fn code_found() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let bytecode = vec![0x60, 0x42, 0x60, 0x00, 0x55];
        let code_hash = alloy_primitives::keccak256(&bytecode);
        let info = AccountInfo {
            nonce: 0,
            balance: U256::ZERO,
            code_hash: code_hash.into(),
        };
        state.db.set_account(&addr, &info).unwrap();
        state.db.set_code(&code_hash.into(), &bytecode).unwrap();
        let router = build_router(state);

        let (status, json) = request(
            &router,
            "/v1/code/0x0000000000000000000000000000000000C0FFEE",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["code"].as_str().unwrap(), "0x6042600055");
    }

    #[tokio::test]
    async fn code_account_not_found() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let (status, _) = request(
            &router,
            "/v1/code/0x0000000000000000000000000000000000000001",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ── POST /v1/batch ──────────────────────────────────────────────

    #[tokio::test]
    async fn batch_mixed_reads() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let info = AccountInfo {
            nonce: 10,
            balance: U256::from(999u64),
            code_hash: B256::ZERO,
        };
        state.db.set_account(&addr, &info).unwrap();
        let slot = B256::ZERO;
        state
            .db
            .set_storage(&addr, &slot, &U256::from(0x42u64))
            .unwrap();
        let router = build_router(state);

        let (status, json) = post_json(
            &router,
            "/v1/batch",
            serde_json::json!([
                { "type": "account", "addr": "0x0000000000000000000000000000000000C0FFEE" },
                { "type": "storage", "addr": "0x0000000000000000000000000000000000C0FFEE", "slot": format!("0x{:064x}", 0u64) },
            ]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["nonce"], 10);
        assert_eq!(arr[1]["value"].as_str().unwrap(), "0x42");
    }

    #[tokio::test]
    async fn batch_empty() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let (status, json) = post_json(&router, "/v1/batch", serde_json::json!([])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn batch_partial_missing() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        state
            .db
            .set_account(
                &addr,
                &AccountInfo {
                    nonce: 1,
                    balance: U256::ZERO,
                    code_hash: B256::ZERO,
                },
            )
            .unwrap();
        let router = build_router(state);

        let (status, json) = post_json(
            &router,
            "/v1/batch",
            serde_json::json!([
                { "type": "account", "addr": "0x0000000000000000000000000000000000C0FFEE" },
                { "type": "account", "addr": "0x0000000000000000000000000000000000000001" },
            ]),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["nonce"], 1);
        assert_eq!(arr[1]["error"].as_str().unwrap(), "not found");
    }

    #[tokio::test]
    async fn batch_large() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let items: Vec<serde_json::Value> = (0..1000)
            .map(|i| {
                serde_json::json!({
                    "type": "account",
                    "addr": format!("0x{:040x}", i),
                })
            })
            .collect();

        let (status, json) = post_json(&router, "/v1/batch", serde_json::json!(items)).await;
        assert_eq!(status, StatusCode::OK);
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1000);
        for item in arr {
            assert_eq!(item["error"].as_str().unwrap(), "not found");
        }
    }

    #[tokio::test]
    async fn batch_malformed_item() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/batch")
            .header("Content-Type", "application/json")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!([
                    { "type": "unknown", "addr": "0x01" }
                ]))
                .unwrap(),
            ))
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ── POST /v1/prefetch ───────────────────────────────────────────

    #[tokio::test]
    async fn prefetch_sload_contract() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(100).unwrap();

        // Contract: PUSH1 0x00  SLOAD  PUSH1 0x00  MSTORE  PUSH1 0x20  PUSH1 0x00  RETURN
        // Reads slot 0 and returns its value as 32 bytes.
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
                &AccountInfo {
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

        let router = build_router(state);
        let (status, json) = post_json(
            &router,
            "/v1/prefetch",
            serde_json::json!({
                "to": "0x0000000000000000000000000000000000C0FFEE",
                "data": "0x"
            }),
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["success"].as_bool().unwrap());
        // Result should contain 0xCAFE (as 32-byte padded)
        let result = json["result"].as_str().unwrap().to_lowercase();
        assert!(result.contains("cafe"));

        // Access list should include the contract
        let al = json["access_list"].as_array().unwrap();
        let contract_entry = al
            .iter()
            .find(|e| {
                e["address"]
                    .as_str()
                    .unwrap()
                    .to_lowercase()
                    .contains("c0ffee")
            })
            .expect("contract not in access list");
        // Should have slot 0 recorded
        assert!(!contract_entry["slots"].as_array().unwrap().is_empty());

        // State slice should have the contract's storage
        assert!(!json["state_slice"]["storage"].as_object().unwrap().is_empty());
        // State slice should have accounts
        assert!(!json["state_slice"]["accounts"].as_object().unwrap().is_empty());
        // State slice should have code
        assert!(!json["state_slice"]["code"].as_object().unwrap().is_empty());
    }

    #[tokio::test]
    async fn prefetch_nonexistent_contract() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(1).unwrap();
        let router = build_router(state);

        let (status, json) = post_json(
            &router,
            "/v1/prefetch",
            serde_json::json!({
                "to": "0x0000000000000000000000000000000000DEAD01",
                "data": "0x"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        // Call to empty address succeeds but returns empty result
        assert_eq!(json["result"].as_str().unwrap(), "0x");
    }

    #[tokio::test]
    async fn prefetch_invalid_data() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(1).unwrap();
        let router = build_router(state);

        let (status, json) = post_json(
            &router,
            "/v1/prefetch",
            serde_json::json!({
                "to": "0x0000000000000000000000000000000000DEAD01",
                "data": "not_hex"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid hex"));
    }

    // ── WebSocket (process_ws_message unit tests) ─────────────────

    #[test]
    fn ws_query_head() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(42).unwrap();

        let resp = process_ws_message(r#"{"type":"head"}"#, &state);
        assert!(resp.id.is_none());
        match &resp.payload {
            WsPayload::Result { result } => assert_eq!(result["head_block"], 42),
            WsPayload::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn ws_query_account() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        state
            .db
            .set_account(
                &addr,
                &AccountInfo {
                    nonce: 7,
                    balance: U256::from(100u64),
                    code_hash: B256::ZERO,
                },
            )
            .unwrap();

        let resp = process_ws_message(
            r#"{"type":"account","addr":"0x0000000000000000000000000000000000C0FFEE"}"#,
            &state,
        );
        match &resp.payload {
            WsPayload::Result { result } => {
                assert_eq!(result["nonce"], 7);
                assert!(result["balance"].as_str().unwrap().starts_with("0x"));
            }
            WsPayload::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn ws_query_storage() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        state
            .db
            .set_storage(&addr, &B256::ZERO, &U256::from(0xBEEFu64))
            .unwrap();

        let msg = format!(
            r#"{{"type":"storage","addr":"0x0000000000000000000000000000000000C0FFEE","slot":"0x{:064x}"}}"#,
            0u64
        );
        let resp = process_ws_message(&msg, &state);
        match &resp.payload {
            WsPayload::Result { result } => {
                assert_eq!(result["value"].as_str().unwrap(), "0xbeef");
            }
            WsPayload::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn ws_query_code() {
        let (_dir, state) = tmp_state();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let bytecode = vec![0x60, 0x42];
        let code_hash = alloy_primitives::keccak256(&bytecode);
        state
            .db
            .set_account(
                &addr,
                &AccountInfo {
                    nonce: 0,
                    balance: U256::ZERO,
                    code_hash: code_hash.into(),
                },
            )
            .unwrap();
        state.db.set_code(&code_hash.into(), &bytecode).unwrap();

        let resp = process_ws_message(
            r#"{"type":"code","addr":"0x0000000000000000000000000000000000C0FFEE"}"#,
            &state,
        );
        match &resp.payload {
            WsPayload::Result { result } => {
                assert_eq!(result["code"].as_str().unwrap(), "0x6042");
            }
            WsPayload::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn ws_not_found() {
        let (_dir, state) = tmp_state();

        let resp = process_ws_message(
            r#"{"type":"account","addr":"0x0000000000000000000000000000000000000001"}"#,
            &state,
        );
        match &resp.payload {
            WsPayload::Error { error } => assert_eq!(error, "not found"),
            WsPayload::Result { .. } => panic!("expected error"),
        }
    }

    #[test]
    fn ws_malformed_message() {
        let (_dir, state) = tmp_state();

        let resp = process_ws_message("not json at all", &state);
        match &resp.payload {
            WsPayload::Error { error } => assert!(error.contains("invalid request")),
            WsPayload::Result { .. } => panic!("expected error"),
        }
    }

    #[test]
    fn ws_with_id() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(99).unwrap();

        let resp = process_ws_message(r#"{"id":42,"type":"head"}"#, &state);
        assert_eq!(resp.id, Some(serde_json::json!(42)));
        match &resp.payload {
            WsPayload::Result { result } => assert_eq!(result["head_block"], 99),
            WsPayload::Error { error } => panic!("unexpected error: {error}"),
        }
    }

    #[test]
    fn ws_with_string_id() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(1).unwrap();

        let resp = process_ws_message(r#"{"id":"req-1","type":"head"}"#, &state);
        assert_eq!(resp.id, Some(serde_json::json!("req-1")));
    }

    #[tokio::test]
    async fn ws_upgrade_handshake() {
        let (_dir, state) = tmp_state();
        let router = build_router(state);

        // A regular GET without upgrade headers should fail
        let req = axum::http::Request::builder()
            .uri("/v1/stream")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        // Without proper WebSocket headers, the upgrade extractor rejects the request
        assert_ne!(resp.status(), StatusCode::OK);
    }

    // ── CORS ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cors_headers_present() {
        let (_dir, state) = tmp_state();
        state.db.set_head_block(1).unwrap();
        let router = build_router(state);

        let req = axum::http::Request::builder()
            .uri("/v1/head")
            .header("Origin", "http://example.com")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert!(resp.headers().contains_key("access-control-allow-origin"));
    }
}
