mod definitions;
mod instrumented_db;
mod middleware;

pub use definitions::*;
pub use instrumented_db::InstrumentedStateDb;
pub use middleware::metrics_middleware;

use axum::http::header::CONTENT_TYPE;
use axum::response::{IntoResponse, Response};
use prometheus::{Encoder, TextEncoder};

/// Axum handler that returns all registered metrics in Prometheus text format.
pub async fn metrics_handler() -> Response {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer).unwrap();

    (
        [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
        buffer,
    )
        .into_response()
}

/// Build a minimal router with just the `/metrics` endpoint.
/// Used by the replayer's standalone metrics server.
pub fn metrics_router() -> axum::Router {
    axum::Router::new().route("/metrics", axum::routing::get(metrics_handler))
}
