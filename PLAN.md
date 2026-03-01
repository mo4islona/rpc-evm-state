# EVM State-as-a-Service — Implementation Plan

## PoC (completed)

- Workspace + shared types (`crates/common`)
- State database with RocksDB (`crates/state-db`)
- Block & transaction data types (`crates/data-types`)
- Chain spec & hardfork config (`crates/chain-spec`) — Ethereum + Polygon with `requires_state_diffs`
- StateDb as revm Database/DatabaseRef trait
- EVM replayer — single block execution + state-diffs mode for Polygon (`crates/replayer`)
- HTTP API — basic endpoints, batch, prefetch, WebSocket (`crates/api`)
- Rust client library — RemoteDB with TrustServer/Verify modes (`crates/client`)
- SQD Network data fetcher with state-diffs support (`crates/sqd-fetcher`)
- Full replay pipeline with progress tracking (`crates/replayer/pipeline`)
- Snapshot import/export with zstd + resume (`crates/snapshot`)
- State validation tool with RPC comparison (`crates/validation`)
- WASM client (`crates/client-wasm`)
- TypeScript SDK with viem transport (`packages/evm-state`)
- Uniswap V3 demo — Rust + TypeScript (`crates/demo`, `packages/demo`)
- Benchmarking suite (`crates/bench`)
- Unified CLI with TUI progress bar (`crates/cli`)

## PoC (not yet implemented)

- **Metrics & Observability** — Prometheus metrics (`/metrics` endpoint), request count/latency by endpoint, DB read latency (p50/p95/p99), state freshness, active WebSocket connections. `crates/api`
- **Rate Limiting & Connection Management** — Per-IP rate limiting via `tower` middleware, WebSocket connection limits, request size limits for batch endpoint, timeouts. `crates/api`

---

## Phase 1 — Replace MDBX with RocksDB (completed)

**Goal:** Drop-in replacement of libmdbx with RocksDB. Same flat key schema, same API surface.

**Decision:** [ADR-001: RocksDB for Versioned State](decisions/001-rocksdb-versioned-state.md)

### Steps

- [x] **Step 1.1:** Replace MDBX with RocksDB — `crates/state-db` rewritten (4 column families, `WriteBatch` `&mut self`, all callers updated)
- [x] **Step 1.2:** Update snapshot, validation, bench, CLI — `WriteBatch` API changes propagated, default db path `./state.rocksdb`
- [ ] **Step 1.3:** RocksDB compression and tuning — per-CF zstd compression, bloom filters (10 bits/key), block cache sizing

---

## Phase 2 — Versioned State (historical queries)

**Goal:** Support historical state queries at any block height.

### Key Schema

```
accounts CF:  [address:20B][inv_block:8B] -> [nonce:8B][balance:32B][code_hash:32B]
storage  CF:  [address:20B][slot:32B][inv_block:8B] -> [value:32B]
code     CF:  [code_hash:32B] -> [bytecode:var]         (unchanged, content-addressed)
metadata CF:  b"head_block" -> [block_number:8B]         (unchanged, singleton)

inv_block = u64::MAX - block_number  (newest sorts first lexicographically)
```

### Steps

#### Step 2.1: Versioned key types
**Crate:** `crates/common` — `src/keys.rs`, `src/lib.rs`

Add `VersionedAccountKey` (28B) and `VersionedStorageKey` (60B) with inverted block height suffix. Add `invert_block_height()` helper. Prefix extractors (20B for accounts, 52B for storage).

#### Step 2.2: Temporal writes — `write_batch(block_number)`
**Crate:** `crates/state-db` + all callers

`WriteBatch` takes block number, writes versioned keys. Deletions become tombstones (empty values). Read methods use prefix-seek for latest.

#### Step 2.3: Historical reads — `get_*_at(block)` methods
**Crate:** `crates/state-db` — `src/lib.rs`, `src/revm_db.rs`

`get_account_at(addr, Option<u64>)`, `get_storage_at(addr, slot, Option<u64>)`. Add `StateDbAtBlock` wrapper for revm `DatabaseRef` at a pinned block.

#### Step 2.4: Update replayer
**Crate:** `crates/replayer`

Thread block number to `db.write_batch(block_number)`.

#### Step 2.5: API historical queries
**Crate:** `crates/api`

Add `?block=N` query param to all endpoints. Backward compatible — omitting = latest.

#### Step 2.6: Configurable retention policy
**Crate:** `crates/state-db`, `crates/cli`

Add a `--retention` CLI option controlling how much history to keep. Default: **latest only** (single snapshot, no history — same as current behavior). Options:
- `latest` — keep only the most recent version of each key (prune older versions after write)
- `N` (blocks) — keep the last N blocks of history, prune versions older than `head - N`
- `full` — keep all history, never prune

Background pruning via RocksDB `DeleteRange` or `CompactRange` with custom filter.

### Verification

1. `cargo test --workspace` at each step
2. Replay to block 1000, query `?block=500` returns correct historical value
3. Retention `latest` produces same DB size as Phase 1 (flat keys)
