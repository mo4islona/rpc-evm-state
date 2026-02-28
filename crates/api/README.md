# evm-state-api

HTTP API for reading EVM state from a [`StateDb`](../state-db). Built with [axum](https://github.com/tokio-rs/axum). Includes a speculative execution endpoint for prefetching state access lists.

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/v1/head` | Current head block number |
| GET | `/v1/account/{addr}` | Account info (nonce, balance, code hash) |
| GET | `/v1/storage/{addr}/{slot}` | Storage slot value |
| GET | `/v1/code/{addr}` | Contract bytecode (looked up via account's code hash) |
| POST | `/v1/batch` | Batch read — multiple queries in one request |
| POST | `/v1/prefetch` | Speculative execution — returns result + access list + state slice |

All responses are JSON with hex-encoded values. Addresses and slots are accepted with or without `0x` prefix.

## Batch endpoint

`POST /v1/batch` accepts a JSON array of requests and returns results in the same order.

```json
[
  { "type": "account", "addr": "0x..." },
  { "type": "storage", "addr": "0x...", "slot": "0x..." },
  { "type": "code",    "addr": "0x..." }
]
```

Each result is the corresponding response object, or `{ "error": "not found" }` for missing data.

## Prefetch endpoint

`POST /v1/prefetch` executes a simulated EVM call and returns all state that was accessed. This lets clients re-execute the call locally without any RPC round-trips.

**Request:**
```json
{ "to": "0x...", "data": "0x...", "from": "0x...", "value": "0x0" }
```

**Response:**
```json
{
  "result": "0x...",
  "success": true,
  "access_list": [
    { "address": "0x...", "slots": ["0x..."] }
  ],
  "state_slice": {
    "accounts": { "0x...": { "nonce": 0, "balance": "0x0", "code_hash": "0x..." } },
    "storage": { "0x...": { "0x...slot": "0x...value" } },
    "code": { "0x...hash": "0x...bytecode" }
  }
}
```

The `state_slice` contains everything needed to re-execute the call locally — account info, storage values, and contract bytecode for all accessed addresses.

## Error codes

- **400** — malformed address, slot, or hex data
- **404** — requested data not found (single endpoints)
- **422** — malformed request body
- **500** — internal database or EVM error

## Usage

```rust
use std::sync::Arc;
use evm_state_api::{build_router, AppState};
use evm_state_db::StateDb;
use evm_state_chain_spec::ChainSpec;

let state = Arc::new(AppState {
    db: StateDb::open("path/to/db")?,
    chain_spec: ChainSpec::polygon_mainnet(),
});
let router = build_router(state);

let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
axum::serve(listener, router).await?;
```

## CORS

All endpoints include permissive CORS headers, allowing requests from any origin.
