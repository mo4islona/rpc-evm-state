# evm-state-api

HTTP API for reading EVM state from a [`StateDb`](../state-db). Built with [axum](https://github.com/tokio-rs/axum).

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/head` | Current head block number |
| GET | `/v1/account/{addr}` | Account info (nonce, balance, code hash) |
| GET | `/v1/storage/{addr}/{slot}` | Storage slot value |
| GET | `/v1/code/{addr}` | Contract bytecode (looked up via account's code hash) |

All responses are JSON with hex-encoded values. Addresses and slots are accepted with or without `0x` prefix.

## Error codes

- **400** — malformed address or slot
- **404** — requested data not found
- **500** — internal database error

## Usage

```rust
use std::sync::Arc;
use evm_state_api::build_router;
use evm_state_db::StateDb;

let db = Arc::new(StateDb::open("path/to/db")?);
let router = build_router(db);

let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
axum::serve(listener, router).await?;
```

## CORS

All endpoints include permissive CORS headers, allowing requests from any origin.
