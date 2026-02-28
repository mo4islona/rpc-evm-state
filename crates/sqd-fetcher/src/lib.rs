use std::time::Duration;

use evm_state_data_types::Block;
use futures::stream::Stream;
use futures::StreamExt;
use serde_json::json;
use tracing::{debug, warn};

// ── Well-known constants ────────────────────────────────────────────

/// Default SQD Network portal base URL.
pub const DEFAULT_PORTAL: &str = "https://portal.sqd.dev";

/// Dataset name for Polygon PoS (chain ID 137).
pub const POLYGON_DATASET: &str = "polygon-mainnet";

/// Dataset name for Ethereum mainnet (chain ID 1).
pub const ETHEREUM_DATASET: &str = "ethereum-mainnet";

// ── Error type ─────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("portal error: {0}")]
    Portal(String),
}

// ── Query builder ──────────────────────────────────────────────────

fn build_query(from: u64, to: Option<u64>, state_diffs: bool) -> serde_json::Value {
    let mut query = json!({
        "type": "evm",
        "fromBlock": from,
        "includeAllBlocks": true,
        "fields": {
            "block": {
                "number": true,
                "hash": true,
                "parentHash": true,
                "timestamp": true,
                "nonce": true,
                "miner": true,
                "difficulty": true,
                "gasLimit": true,
                "gasUsed": true,
                "baseFeePerGas": true,
                "extraData": true,
                "mixHash": true,
                "sha3Uncles": true,
                "stateRoot": true,
                "transactionsRoot": true,
                "receiptsRoot": true,
                "size": true
            }
        }
    });

    if state_diffs {
        query["stateDiffs"] = json!([{}]);
        query["fields"]["stateDiff"] = json!({
            "transactionIndex": true,
            "address": true,
            "key": true,
            "kind": true,
            "next": true
        });
    } else {
        query["transactions"] = json!([{}]);
        query["fields"]["transaction"] = json!({
            "transactionIndex": true,
            "hash": true,
            "from": true,
            "to": true,
            "input": true,
            "value": true,
            "nonce": true,
            "gas": true,
            "gasPrice": true,
            "maxFeePerGas": true,
            "maxPriorityFeePerGas": true,
            "yParity": true,
            "chainId": true,
            "gasUsed": true,
            "cumulativeGasUsed": true,
            "effectiveGasPrice": true,
            "contractAddress": true,
            "type": true,
            "status": true,
            "sighash": true
        });
    }

    if let Some(to) = to {
        query["toBlock"] = json!(to);
    }
    query
}

// ── Helpers ───────────────────────────────────────────────────────

/// Parse a single NDJSON line into a Block with a clear error message.
fn parse_block_line(line: &str) -> Result<Block, Error> {
    // Detect JSON array format (wrong) on first non-empty line.
    if line.starts_with('[') {
        return Err(Error::Portal(
            "portal returned a JSON array, but expected newline-delimited JSON (NDJSON)".into(),
        ));
    }

    serde_json::from_str(line).map_err(|e| {
        Error::Portal(format!(
            "failed to parse block: {e}\n  content: {}",
            &line[..line.len().min(200)]
        ))
    })
}

// ── SqdFetcher ─────────────────────────────────────────────────────

const MAX_RETRIES: u32 = 5;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Fetches blocks from the Subsquid Network via the portal API.
///
/// The portal returns blocks as newline-delimited JSON (NDJSON). Each line
/// is one JSON object representing a block. The response is streamed — blocks
/// are parsed and yielded as bytes arrive, providing natural backpressure.
///
/// When the portal closes the connection (end of a chunk), the fetcher
/// automatically re-requests from the last received block + 1 to continue
/// pagination.
pub struct SqdFetcher {
    client: reqwest::Client,
    portal_url: String,
    use_state_diffs: bool,
}

impl SqdFetcher {
    /// Create a new fetcher for a dataset on the given portal.
    ///
    /// Constructs the stream URL as `{portal_url}/datasets/{dataset}/stream`.
    ///
    /// # Example
    /// ```ignore
    /// let fetcher = SqdFetcher::new(DEFAULT_PORTAL, POLYGON_DATASET);
    /// ```
    pub fn new(portal_url: &str, dataset: &str) -> Self {
        let base = portal_url.trim_end_matches('/');
        Self::from_url(&format!("{base}/datasets/{dataset}/stream"))
    }

    /// Create a new fetcher pointing at an explicit stream URL.
    pub fn from_url(stream_url: &str) -> Self {
        Self {
            client: reqwest::Client::new(),
            portal_url: stream_url.trim_end_matches('/').to_string(),
            use_state_diffs: false,
        }
    }

    /// Enable state-diffs mode: query `stateDiffs` instead of `transactions`.
    pub fn with_state_diffs(mut self, enabled: bool) -> Self {
        self.use_state_diffs = enabled;
        self
    }

    /// Start a request with retries on transient errors (server 5xx, timeouts,
    /// connection failures). Returns the response ready for streaming.
    async fn start_request(
        &self,
        from: u64,
        to: Option<u64>,
    ) -> Result<reqwest::Response, Error> {
        let query = build_query(from, to, self.use_state_diffs);

        let mut last_error = None;
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = INITIAL_BACKOFF * 2u32.pow(attempt - 1);
                warn!(attempt, backoff_ms = backoff.as_millis(), "retrying portal request");
                tokio::time::sleep(backoff).await;
            }

            debug!(from, to = to.map(|t| t.to_string()).unwrap_or_default(), "requesting blocks from portal");

            match self
                .client
                .post(&self.portal_url)
                .json(&query)
                .send()
                .await
            {
                Ok(resp) => {
                    if resp.status().is_server_error() {
                        last_error =
                            Some(Error::Portal(format!("portal returned {}", resp.status())));
                        continue;
                    }
                    let resp = resp
                        .error_for_status()
                        .map_err(|e| Error::Portal(format!("portal error: {e}")))?;
                    return Ok(resp);
                }
                Err(e) if e.is_timeout() || e.is_connect() => {
                    last_error = Some(e.into());
                    continue;
                }
                Err(e) => return Err(e.into()),
            }
        }

        Err(last_error.unwrap_or_else(|| Error::Portal("max retries exceeded".into())))
    }

    /// Stream blocks from the SQD Network in a given range.
    ///
    /// Yields blocks one at a time as they arrive from the portal. The response
    /// is streamed line-by-line — blocks are parsed and yielded immediately,
    /// providing natural TCP-level backpressure when the consumer is slow.
    ///
    /// When the portal closes the connection (end of a chunk), the fetcher
    /// automatically re-requests from `last_block + 1` to continue. The stream
    /// ends when:
    /// - The `to` block is reached, or
    /// - The portal returns an empty response (dataset exhausted).
    pub fn stream_blocks(
        &self,
        from: u64,
        to: Option<u64>,
    ) -> impl Stream<Item = Result<Block, Error>> + '_ {
        async_stream::try_stream! {
            let mut current = from;

            loop {
                // Check if we've passed the end.
                if let Some(to) = to {
                    if current > to {
                        break;
                    }
                }

                // Start a new request from `current`.
                let resp = self.start_request(current, to).await?;
                let mut byte_stream = resp.bytes_stream();

                let mut buffer = Vec::new();
                let mut last_block: Option<u64> = None;
                let mut got_any = false;

                // Stream the response body, parsing blocks as complete lines arrive.
                while let Some(chunk) = byte_stream.next().await {
                    let chunk: bytes::Bytes = chunk.map_err(Error::Http)?;
                    buffer.extend_from_slice(&chunk);

                    // Process all complete lines in the buffer.
                    while let Some(newline_pos) = buffer.iter().position(|&b| b == b'\n') {
                        let line = std::str::from_utf8(&buffer[..newline_pos])
                            .map_err(|e| Error::Portal(format!("invalid UTF-8 in response: {e}")))?
                            .trim();

                        if !line.is_empty() {
                            let block = parse_block_line(line)?;
                            last_block = Some(block.header.number);
                            got_any = true;
                            yield block;
                        }

                        buffer.drain(..=newline_pos);
                    }
                }

                // Handle any trailing data (last line without newline).
                if !buffer.is_empty() {
                    let line = std::str::from_utf8(&buffer)
                        .map_err(|e| Error::Portal(format!("invalid UTF-8 in response: {e}")))?
                        .trim();

                    if !line.is_empty() {
                        let block = parse_block_line(line)?;
                        last_block = Some(block.header.number);
                        got_any = true;
                        yield block;
                    }
                }

                // Empty response means the dataset is exhausted.
                if !got_any {
                    break;
                }

                // Check if we've reached the end.
                if to.is_some_and(|to| last_block.unwrap() >= to) {
                    break;
                }

                // Paginate: continue from the next block.
                current = last_block.unwrap() + 1;
                debug!(from = current, "portal chunk done, requesting next");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use axum::routing::post;
    use axum::Router;
    use futures::StreamExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    // ── parse_block_line tests ────────────────────────────────────

    #[test]
    fn parse_block_line_valid() {
        let block = parse_block_line(r#"{"header":{"number":42}}"#).unwrap();
        assert_eq!(block.header.number, 42);
    }

    #[test]
    fn parse_block_line_rejects_json_array() {
        let err = parse_block_line(r#"[{"header":{"number":1}}]"#).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("JSON array"), "got: {msg}");
        assert!(msg.contains("NDJSON"), "got: {msg}");
    }

    #[test]
    fn parse_block_line_invalid_json() {
        let err = parse_block_line("not valid json").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not valid json"), "got: {msg}");
    }

    #[test]
    fn parse_block_line_missing_field() {
        let err = parse_block_line(r#"{"wrong_field":123}"#).unwrap_err();
        assert!(err.to_string().contains("failed to parse block"));
    }

    // ── Query builder tests ────────────────────────────────────────

    #[test]
    fn build_query_without_to() {
        let query = build_query(100, None, false);
        assert_eq!(query["type"], "evm");
        assert_eq!(query["fromBlock"], 100);
        assert!(query.get("toBlock").is_none());
        assert_eq!(query["includeAllBlocks"], true);
        assert!(query["transactions"].is_array());
        assert!(query["fields"]["block"]["hash"].as_bool().unwrap());
        assert!(query["fields"]["transaction"]["from"].as_bool().unwrap());
    }

    #[test]
    fn build_query_with_to() {
        let query = build_query(100, Some(200), false);
        assert_eq!(query["type"], "evm");
        assert_eq!(query["fromBlock"], 100);
        assert_eq!(query["toBlock"], 200);
    }

    #[test]
    fn build_query_state_diffs_mode() {
        let query = build_query(100, None, true);
        assert_eq!(query["type"], "evm");
        assert!(query["stateDiffs"].is_array());
        assert!(query.get("transactions").is_none());
        assert!(query["fields"]["stateDiff"]["address"].as_bool().unwrap());
        assert!(query["fields"]["stateDiff"]["key"].as_bool().unwrap());
        assert!(query["fields"]["stateDiff"]["kind"].as_bool().unwrap());
        assert!(query["fields"]["stateDiff"]["next"].as_bool().unwrap());
        assert!(query["fields"].get("transaction").is_none());
    }

    #[test]
    fn deserialize_block_response() {
        let fixture = include_str!("../../data-types/fixtures/polygon_block_65000000.json");
        let json = format!("[{fixture}]");
        let blocks: Vec<Block> = serde_json::from_str(&json).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].header.number, 65_000_000);
        assert_eq!(blocks[0].transactions.len(), 3);
    }

    // ── Mock server helpers ────────────────────────────────────────

    /// Convert a list of JSON values to newline-delimited JSON (matching the real portal format).
    fn to_ndjson(values: &[&serde_json::Value]) -> String {
        values
            .iter()
            .map(|v| serde_json::to_string(v).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Builds a mock portal that serves blocks filtered by fromBlock/toBlock.
    fn mock_portal(blocks: Vec<serde_json::Value>) -> Router {
        let blocks = Arc::new(blocks);

        Router::new().route(
            "/stream",
            post(move |body: axum::Json<serde_json::Value>| {
                let blocks = blocks.clone();
                async move {
                    let from = body["fromBlock"].as_u64().unwrap();
                    let to = body.get("toBlock").and_then(|v| v.as_u64());

                    let result: Vec<&serde_json::Value> = blocks
                        .iter()
                        .filter(|b| {
                            let n = b["header"]["number"].as_u64().unwrap();
                            n >= from && to.map_or(true, |t| n <= t)
                        })
                        .collect();

                    to_ndjson(&result).into_response()
                }
            }),
        )
    }

    /// Builds a mock portal that returns at most `chunk_size` blocks per request
    /// (simulating the portal closing the connection after a chunk).
    fn mock_portal_chunked(blocks: Vec<serde_json::Value>, chunk_size: usize) -> Router {
        let blocks = Arc::new(blocks);

        Router::new().route(
            "/stream",
            post(move |body: axum::Json<serde_json::Value>| {
                let blocks = blocks.clone();
                async move {
                    let from = body["fromBlock"].as_u64().unwrap();
                    let to = body.get("toBlock").and_then(|v| v.as_u64());

                    let result: Vec<&serde_json::Value> = blocks
                        .iter()
                        .filter(|b| {
                            let n = b["header"]["number"].as_u64().unwrap();
                            n >= from && to.map_or(true, |t| n <= t)
                        })
                        .take(chunk_size)
                        .collect();

                    to_ndjson(&result).into_response()
                }
            }),
        )
    }

    async fn start_mock(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}/stream")
    }

    fn make_block(number: u64) -> serde_json::Value {
        serde_json::json!({
            "header": { "number": number },
            "transactions": []
        })
    }

    // ── Integration tests with mock server ─────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stream_all_blocks() {
        let blocks = vec![make_block(10), make_block(11), make_block(12)];
        let url = start_mock(mock_portal(blocks)).await;
        let fetcher = SqdFetcher::from_url(&url);

        let result: Vec<Block> = fetcher
            .stream_blocks(10, Some(12))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].header.number, 10);
        assert_eq!(result[1].header.number, 11);
        assert_eq!(result[2].header.number, 12);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stream_subset() {
        let blocks = vec![
            make_block(10),
            make_block(11),
            make_block(12),
            make_block(13),
            make_block(14),
        ];
        let url = start_mock(mock_portal(blocks)).await;
        let fetcher = SqdFetcher::from_url(&url);

        let result: Vec<Block> = fetcher
            .stream_blocks(11, Some(13))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(result.len(), 3);
        assert_eq!(result[0].header.number, 11);
        assert_eq!(result[2].header.number, 13);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn empty_response_stops_stream() {
        let blocks: Vec<serde_json::Value> = vec![];
        let url = start_mock(mock_portal(blocks)).await;
        let fetcher = SqdFetcher::from_url(&url);

        let result: Vec<Block> = fetcher
            .stream_blocks(100, Some(200))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn retry_on_server_error() {
        let call_count = Arc::new(AtomicU32::new(0));
        let call_count_query = call_count.clone();

        let router = Router::new().route(
            "/stream",
            post(move || {
                let count = call_count_query.clone();
                async move {
                    let n = count.fetch_add(1, Ordering::SeqCst);
                    if n < 2 {
                        (
                            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                            "server error",
                        )
                            .into_response()
                    } else {
                        serde_json::to_string(&make_block(100))
                            .unwrap()
                            .into_response()
                    }
                }
            }),
        );

        let url = start_mock(router).await;
        let fetcher = SqdFetcher::from_url(&url);

        let result: Vec<Block> = fetcher
            .stream_blocks(100, Some(100))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].header.number, 100);
        assert!(call_count.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pagination_across_chunks() {
        let blocks: Vec<serde_json::Value> = (10..=14).map(make_block).collect();
        // Portal returns at most 2 blocks per request — fetcher must paginate.
        let url = start_mock(mock_portal_chunked(blocks, 2)).await;
        let fetcher = SqdFetcher::from_url(&url);

        let result: Vec<Block> = fetcher
            .stream_blocks(10, Some(14))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // Should get all 5 blocks across 3 requests: [10,11], [12,13], [14]
        assert_eq!(result.len(), 5);
        for (i, block) in result.iter().enumerate() {
            assert_eq!(block.header.number, 10 + i as u64);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn open_ended_stream_paginates() {
        let blocks: Vec<serde_json::Value> = (0..=5).map(make_block).collect();
        // Portal returns at most 3 blocks per request, no toBlock.
        let url = start_mock(mock_portal_chunked(blocks, 3)).await;
        let fetcher = SqdFetcher::from_url(&url);

        let result: Vec<Block> = fetcher
            .stream_blocks(0, None)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // Should get all 6 blocks across 3 requests: [0,1,2], [3,4,5], [] (empty stops)
        assert_eq!(result.len(), 6);
        for (i, block) in result.iter().enumerate() {
            assert_eq!(block.header.number, i as u64);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn query_includes_type_evm() {
        let received_query = Arc::new(tokio::sync::Mutex::new(None));
        let received_clone = received_query.clone();

        let router = Router::new().route(
            "/stream",
            post(move |body: axum::Json<serde_json::Value>| {
                let received = received_clone.clone();
                async move {
                    *received.lock().await = Some(body.0.clone());
                    "".into_response()
                }
            }),
        );

        let url = start_mock(router).await;
        let fetcher = SqdFetcher::from_url(&url);

        let _: Vec<Block> = fetcher
            .stream_blocks(100, Some(200))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        let query = received_query.lock().await;
        let query = query.as_ref().unwrap();
        assert_eq!(query["type"], "evm");
        assert_eq!(query["fromBlock"], 100);
        assert_eq!(query["toBlock"], 200);
        assert_eq!(query["includeAllBlocks"], true);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn json_array_response_gives_clear_error() {
        // Simulate a portal that returns a JSON array instead of NDJSON.
        let router = Router::new().route(
            "/stream",
            post(|| async {
                axum::Json(vec![make_block(1), make_block(2)]).into_response()
            }),
        );

        let url = start_mock(router).await;
        let fetcher = SqdFetcher::from_url(&url);

        let results: Vec<std::result::Result<Block, Error>> =
            fetcher.stream_blocks(1, Some(2)).collect().await;

        assert_eq!(results.len(), 1);
        let err = results[0].as_ref().unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("JSON array"),
            "expected clear error about JSON array, got: {msg}"
        );
    }

    // ── Integration test against the real SQD portal ──────────────

    /// Fetches a real block with transactions from the public SQD portal
    /// and verifies all required fields deserialize correctly. This catches
    /// mismatches between our query fields and the Block/Transaction structs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore] // requires network — run with: cargo test -p evm-state-sqd-fetcher -- --ignored
    async fn portal_returns_all_required_fields() {
        let fetcher = SqdFetcher::new(DEFAULT_PORTAL, POLYGON_DATASET);

        // Block 65_000_000 has 3 transactions (matches our fixture).
        let blocks: Vec<Block> = fetcher
            .stream_blocks(65_000_000, Some(65_000_000))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("portal response should deserialize without errors");

        assert_eq!(blocks.len(), 1, "expected exactly 1 block");
        let block = &blocks[0];

        // Header required fields.
        assert_eq!(block.header.number, 65_000_000);
        assert!(block.header.hash.is_some(), "hash missing");
        assert!(block.header.parent_hash.is_some(), "parentHash missing");
        assert!(block.header.timestamp.is_some(), "timestamp missing");
        assert!(block.header.gas_limit.is_some(), "gasLimit missing");
        assert!(block.header.gas_used.is_some(), "gasUsed missing");
        assert!(block.header.miner.is_some(), "miner missing");

        // Transactions — block 65M has many txs; just verify they all parsed.
        assert!(!block.transactions.is_empty(), "expected transactions");
        for (i, tx) in block.transactions.iter().enumerate() {
            assert_eq!(
                tx.transaction_index, i as u32,
                "transactionIndex mismatch for tx {i}"
            );
            assert!(tx.hash.is_some(), "tx {i}: hash missing");
            assert!(tx.from.is_some(), "tx {i}: from missing");
            assert!(tx.gas.is_some(), "tx {i}: gas missing");
            assert!(tx.gas_used.is_some(), "tx {i}: gasUsed missing");
            assert!(tx.status.is_some(), "tx {i}: status missing");
            assert!(tx.tx_type.is_some(), "tx {i}: type missing");
        }
    }
}
