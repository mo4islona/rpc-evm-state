use std::time::Instant;

use axum::extract::MatchedPath;
use axum::http::Request;
use axum::middleware::Next;
use axum::response::Response;

use crate::definitions::{HTTP_REQUESTS_TOTAL, HTTP_REQUEST_DURATION};

/// Axum middleware that records request count and latency into Prometheus.
///
/// Uses [`MatchedPath`] for the route template (e.g. `/v1/account/{addr}`)
/// to avoid unbounded label cardinality from hex addresses in paths.
pub async fn metrics_middleware(
    matched_path: Option<MatchedPath>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = matched_path
        .map(|mp| mp.as_str().to_string())
        .unwrap_or_else(|| req.uri().path().to_string());
    let method = req.method().to_string();
    let start = Instant::now();

    let response = next.run(req).await;

    let status = response.status().as_u16().to_string();
    HTTP_REQUEST_DURATION
        .with_label_values(&[&method, &path])
        .observe(start.elapsed().as_secs_f64());
    HTTP_REQUESTS_TOTAL
        .with_label_values(&[&method, &path, &status])
        .inc();

    response
}
