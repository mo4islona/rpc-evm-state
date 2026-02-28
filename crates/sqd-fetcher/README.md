# evm-state-sqd-fetcher

Async block fetcher for the Subsquid Network. Streams blocks and transactions from the SQD portal API, handling pagination and retries transparently.

## Features

- **`SqdFetcher`** — async client that fetches EVM blocks from the SQD portal
- **Streaming** — `stream_blocks()` returns an async `Stream<Item = Result<Block, Error>>` with automatic pagination
- **Retry logic** — transient errors (5xx, timeout, connection) are retried up to 3 times with exponential backoff
- **Configurable portal** — the portal base URL defaults to `https://portal.sqd.dev` but can be overridden
- **Well-known datasets** — constants for Polygon and Ethereum mainnet

## Usage

```rust
use evm_state_sqd_fetcher::{SqdFetcher, DEFAULT_PORTAL, POLYGON_DATASET};
use futures::StreamExt;

let fetcher = SqdFetcher::new(DEFAULT_PORTAL, POLYGON_DATASET);

// Stream blocks 65_000_000..=65_000_100
let mut stream = fetcher.stream_blocks(65_000_000, Some(65_000_100));
while let Some(block) = stream.next().await {
    let block = block?;
    println!("block {} with {} txs", block.header.number, block.transactions.len());
}

// Or use a custom portal URL
let fetcher = SqdFetcher::new("https://my-portal.example.com", POLYGON_DATASET);
```

## How it works

`SqdFetcher` wraps `reqwest` and POSTs queries to the portal's `/datasets/{dataset}/stream` endpoint. Each request includes `"type": "evm"` and field selectors for all block header and transaction fields needed for EVM execution.

The portal returns a JSON array of blocks. For large ranges, the fetcher paginates automatically: it uses the last block's number as a cursor and issues follow-up requests until the target range is covered or the dataset is exhausted.

## Well-known datasets

| Network           | Dataset             | Constant           |
|-------------------|---------------------|--------------------|
| Polygon PoS       | `polygon-mainnet`   | `POLYGON_DATASET`  |
| Ethereum mainnet  | `ethereum-mainnet`  | `ETHEREUM_DATASET` |
