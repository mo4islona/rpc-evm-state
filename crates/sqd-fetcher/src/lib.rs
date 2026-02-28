use std::time::Duration;

use evm_state_data_types::Block;
use futures::stream::{self, Stream};
use futures::TryStreamExt;
use serde_json::json;

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

fn build_query(from: u64, to: Option<u64>) -> serde_json::Value {
    let mut query = json!({
        "type": "evm",
        "fromBlock": from,
        "includeAllBlocks": true,
        "transactions": [{}],
        "fields": {
            "block": {
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
            },
            "transaction": {
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
            }
        }
    });
    if let Some(to) = to {
        query["toBlock"] = json!(to);
    }
    query
}

// ── SqdFetcher ─────────────────────────────────────────────────────

const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Fetches blocks from the Subsquid Network via the portal API.
///
/// POSTs queries to the portal's `/stream` endpoint and handles pagination
/// by using the last block number as the cursor for the next chunk.
pub struct SqdFetcher {
    client: reqwest::Client,
    portal_url: String,
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
        }
    }

    /// Fetch a chunk of blocks from the portal.
    ///
    /// Returns the parsed blocks. The caller uses the last block's number
    /// to determine where to continue from.
    async fn fetch_chunk(&self, from: u64, to: Option<u64>) -> Result<Vec<Block>, Error> {
        let query = build_query(from, to);

        let mut last_error = None;
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                let backoff = INITIAL_BACKOFF * 2u32.pow(attempt - 1);
                tokio::time::sleep(backoff).await;
            }

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
                    let blocks: Vec<Block> = resp.json().await?;
                    return Ok(blocks);
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
    /// Yields blocks one at a time, transparently handling pagination
    /// across multiple requests. The stream ends when the `to` block
    /// is reached or the dataset is exhausted.
    pub fn stream_blocks(
        &self,
        from: u64,
        to: Option<u64>,
    ) -> impl Stream<Item = Result<Block, Error>> + '_ {
        stream::try_unfold(
            (from, false),
            move |(current, done)| async move {
                if done {
                    return Ok::<_, Error>(None);
                }

                if let Some(to) = to {
                    if current > to {
                        return Ok::<_, Error>(None);
                    }
                }

                let blocks = self.fetch_chunk(current, to).await?;

                if blocks.is_empty() {
                    return Ok(None);
                }

                let last_block = blocks.last().unwrap().header.number;
                let reached_end = to.is_some_and(|to| last_block >= to);

                Ok(Some((
                    stream::iter(blocks.into_iter().map(Ok)),
                    (last_block + 1, reached_end),
                )))
            },
        )
        .try_flatten()
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

    // ── Query builder tests ────────────────────────────────────────

    #[test]
    fn build_query_without_to() {
        let query = build_query(100, None);
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
        let query = build_query(100, Some(200));
        assert_eq!(query["type"], "evm");
        assert_eq!(query["fromBlock"], 100);
        assert_eq!(query["toBlock"], 200);
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

                    axum::Json(result).into_response()
                }
            }),
        )
    }

    /// Builds a mock portal that returns at most `chunk_size` blocks per request.
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

                    axum::Json(result).into_response()
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
                        axum::Json(vec![make_block(100)]).into_response()
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
        let url = start_mock(mock_portal_chunked(blocks, 2)).await;
        let fetcher = SqdFetcher::from_url(&url);

        let result: Vec<Block> = fetcher
            .stream_blocks(10, Some(14))
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        // Should get all 5 blocks across 3 chunks: [10,11], [12,13], [14]
        assert_eq!(result.len(), 5);
        for (i, block) in result.iter().enumerate() {
            assert_eq!(block.header.number, 10 + i as u64);
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
                    axum::Json(Vec::<serde_json::Value>::new()).into_response()
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
}
