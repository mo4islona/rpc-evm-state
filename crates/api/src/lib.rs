use std::sync::Arc;

use alloy_primitives::{hex, Address, B256};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use evm_state_db::StateDb;
use serde::Serialize;
use tower_http::cors::CorsLayer;

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

async fn get_head(State(db): State<Arc<StateDb>>) -> AppResult<impl IntoResponse> {
    let head = db.get_head_block()?.ok_or(AppError::NotFound)?;
    Ok(Json(HeadResponse { head_block: head }))
}

async fn get_account(
    State(db): State<Arc<StateDb>>,
    Path(addr): Path<String>,
) -> AppResult<impl IntoResponse> {
    let address = parse_address(&addr)?;
    let info = db.get_account(&address)?.ok_or(AppError::NotFound)?;
    Ok(Json(AccountResponse {
        nonce: info.nonce,
        balance: format!("{:#x}", info.balance),
        code_hash: format!("{:#x}", info.code_hash),
    }))
}

async fn get_storage(
    State(db): State<Arc<StateDb>>,
    Path((addr, slot)): Path<(String, String)>,
) -> AppResult<impl IntoResponse> {
    let address = parse_address(&addr)?;
    let slot = parse_b256(&slot)?;
    let value = db.get_storage(&address, &slot)?.ok_or(AppError::NotFound)?;
    Ok(Json(StorageResponse {
        value: format!("{:#x}", value),
    }))
}

async fn get_code(
    State(db): State<Arc<StateDb>>,
    Path(addr): Path<String>,
) -> AppResult<impl IntoResponse> {
    let address = parse_address(&addr)?;
    let info = db.get_account(&address)?.ok_or(AppError::NotFound)?;
    let code = db.get_code(&info.code_hash)?.ok_or(AppError::NotFound)?;
    Ok(Json(CodeResponse {
        code: format!("0x{}", hex::encode(&code)),
    }))
}

// ── Router ──────────────────────────────────────────────────────────

/// Build the axum [`Router`] with all v1 endpoints.
///
/// The caller provides a shared [`StateDb`] wrapped in an [`Arc`].
pub fn build_router(db: Arc<StateDb>) -> Router {
    Router::new()
        .route("/v1/head", get(get_head))
        .route("/v1/account/{addr}", get(get_account))
        .route("/v1/storage/{addr}/{slot}", get(get_storage))
        .route("/v1/code/{addr}", get(get_code))
        .layer(CorsLayer::permissive())
        .with_state(db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use evm_state_common::AccountInfo;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn tmp_db() -> (tempfile::TempDir, Arc<StateDb>) {
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(StateDb::open(dir.path()).unwrap());
        (dir, db)
    }

    async fn request(
        router: &Router,
        uri: &str,
    ) -> (StatusCode, serde_json::Value) {
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

    // ── /v1/head ────────────────────────────────────────────────────

    #[tokio::test]
    async fn head_block_returns_value() {
        let (_dir, db) = tmp_db();
        db.set_head_block(65_000_000).unwrap();
        let router = build_router(db);

        let (status, json) = request(&router, "/v1/head").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["head_block"], 65_000_000);
    }

    #[tokio::test]
    async fn head_block_not_set() {
        let (_dir, db) = tmp_db();
        let router = build_router(db);

        let (status, _) = request(&router, "/v1/head").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ── /v1/account/:addr ───────────────────────────────────────────

    #[tokio::test]
    async fn account_found() {
        let (_dir, db) = tmp_db();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let info = AccountInfo {
            nonce: 42,
            balance: alloy_primitives::U256::from(1_000_000u64),
            code_hash: B256::ZERO,
        };
        db.set_account(&addr, &info).unwrap();
        let router = build_router(db);

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
        let (_dir, db) = tmp_db();
        let router = build_router(db);

        let (status, _) = request(
            &router,
            "/v1/account/0x0000000000000000000000000000000000000001",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn account_invalid_address() {
        let (_dir, db) = tmp_db();
        let router = build_router(db);

        let (status, json) = request(&router, "/v1/account/notanaddr").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid address"));
    }

    // ── /v1/storage/:addr/:slot ─────────────────────────────────────

    #[tokio::test]
    async fn storage_found() {
        let (_dir, db) = tmp_db();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let slot = B256::ZERO;
        let value = alloy_primitives::U256::from(0x42u64);
        db.set_storage(&addr, &slot, &value).unwrap();
        let router = build_router(db);

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
        let (_dir, db) = tmp_db();
        let router = build_router(db);

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
        let (_dir, db) = tmp_db();
        let router = build_router(db);

        let (status, json) = request(&router, "/v1/storage/badaddr/badslot").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(json["error"].as_str().unwrap().contains("invalid"));
    }

    // ── /v1/code/:addr ──────────────────────────────────────────────

    #[tokio::test]
    async fn code_found() {
        let (_dir, db) = tmp_db();
        let addr = "0x0000000000000000000000000000000000C0FFEE"
            .parse::<Address>()
            .unwrap();
        let bytecode = vec![0x60, 0x42, 0x60, 0x00, 0x55];
        let code_hash = alloy_primitives::keccak256(&bytecode);
        let info = AccountInfo {
            nonce: 0,
            balance: alloy_primitives::U256::ZERO,
            code_hash: code_hash.into(),
        };
        db.set_account(&addr, &info).unwrap();
        db.set_code(&code_hash.into(), &bytecode).unwrap();
        let router = build_router(db);

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
        let (_dir, db) = tmp_db();
        let router = build_router(db);

        let (status, _) = request(
            &router,
            "/v1/code/0x0000000000000000000000000000000000000001",
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    // ── CORS ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn cors_headers_present() {
        let (_dir, db) = tmp_db();
        db.set_head_block(1).unwrap();
        let router = build_router(db);

        let req = axum::http::Request::builder()
            .uri("/v1/head")
            .header("Origin", "http://example.com")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert!(resp.headers().contains_key("access-control-allow-origin"));
    }
}
