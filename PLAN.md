# EVM State-as-a-Service — Implementation Plan

Step-by-step plan for building the full PoC described in CONCEPT.md.
Each step is a self-sufficient feature with its own tests.
Every crate must include a `README.md` explaining what it does, why it exists, and how it works.
Steps are grouped by **dependency tier** — everything within the same tier can be done in parallel.

---

## Tier 0 — Foundation (must be first)

### Step 1: Rust Workspace + Shared Types Crate
**Crate**: workspace root + `crates/common`
**Scope**: Initialize Cargo workspace. Create a `common` crate with shared EVM types: `Address`, `B256`, `U256`, account info struct, and DB key encoding/decoding helpers. Re-export `alloy-primitives` types.
**Tests**: Unit tests for key encoding/decoding (round-trip), serialization of account info (nonce + balance + code_hash).
**Depends on**: Nothing

---

## Tier 1 — Core Storage & Data (parallel)

### Step 2: State Database — libmdbx Storage Layer
**Crate**: `crates/state-db`
**Scope**: Implement the flat KV state database using `libmdbx-rs`. Four tables: `storage`, `accounts`, `code`, `metadata`. Implement `StateDb` struct with methods: `get_storage(addr, slot)`, `set_storage(addr, slot, value)`, `get_account(addr)`, `set_account(addr, info)`, `get_code(hash)`, `set_code(hash, bytecode)`, `get_head_block()`, `set_head_block(n)`. Implement batch write support (write multiple changes in a single transaction).
**Tests**: CRUD for each table, batch writes, read-after-write consistency, missing key returns `None`, overwrite existing values, database reopening persists data.
**Depends on**: Step 1

### Step 3: Block & Transaction Data Types + Deserialization
**Crate**: `crates/data-types`
**Scope**: Define Rust types for blocks and transactions as they come from Subsquid Network (JSON format). Implement `serde` deserialization. Include all fields needed for EVM execution: `from`, `to`, `input`, `value`, `gas`, `gas_price`, `nonce`, `type` (legacy/EIP-1559/EIP-2930), `max_fee_per_gas`, `max_priority_fee_per_gas`, `access_list`. Block fields: `number`, `hash`, `parent_hash`, `timestamp`, `gas_limit`, `base_fee_per_gas`, `difficulty`, `coinbase`.
**Tests**: Deserialize sample Polygon blocks from JSON fixtures, verify all fields parse correctly, test edge cases (empty transactions, zero values, missing optional fields).
**Depends on**: Step 1

### Step 4: Chain Spec & Hardfork Configuration
**Crate**: `crates/chain-spec`
**Scope**: Define `ChainSpec` struct that maps block numbers to revm `SpecId` (hardfork levels). Implement Polygon PoS chain spec (chain ID 137, hardfork schedule from London through Cancun). Helper function `spec_for_block(block_number, chain_spec) -> SpecId`. Helper to build revm `BlockEnv` and `TxEnv` from the data types in Step 3.
**Tests**: Correct spec IDs for known Polygon hardfork boundaries, block env construction produces valid revm inputs, chain ID is correctly set.
**Depends on**: Step 1

---

## Tier 2 — Core Engine & API (parallel tracks)

### Step 5: StateDb as revm Database Trait
**Crate**: `crates/state-db` (extend)
**Scope**: Implement `revm::Database` trait for `StateDb`. This allows revm to read state directly from libmdbx. Methods: `basic(address)` → account info, `storage(address, index)` → slot value, `code_by_hash(hash)` → bytecode, `block_hash(number)` → hash. Also implement `DatabaseRef` (immutable variant).
**Tests**: Create a populated `StateDb`, wrap it as a revm `Database`, execute a simple contract call (e.g., return a constant), verify the result. Test missing account returns default. Test storage reads.
**Depends on**: Step 2, Step 4

### Step 6: EVM Replayer — Single Block Execution
**Crate**: `crates/replayer`
**Scope**: Implement block replay: take a `Block` with transactions, execute all transactions in order using revm against `StateDb`, capture resulting state changes (account modifications + storage writes), and commit them to the DB. Handle transaction types (legacy, EIP-1559, EIP-2930). Track `coinbase` rewards.
**Tests**: Replay a known Polygon block (from fixture), verify resulting state changes match expected values. Test with ETH transfers, contract calls, contract deployments. Verify nonce increments, balance changes, gas consumption.
**Depends on**: Step 3, Step 5

### Step 7: HTTP API — Basic Endpoints
**Crate**: `crates/api`
**Scope**: axum HTTP server with endpoints: `GET /v1/storage/:addr/:slot`, `GET /v1/account/:addr`, `GET /v1/code/:addr`, `GET /v1/head`. JSON responses with hex-encoded values. Error handling for invalid addresses/slots, 404 for missing data. CORS headers. Shared `StateDb` via `Arc`.
**Tests**: Integration tests using `axum::test` — query known values, test 404 for missing data, test malformed input handling, test concurrent reads.
**Depends on**: Step 2

### Step 8: HTTP API — Batch Endpoint
**Crate**: `crates/api` (extend)
**Scope**: `POST /v1/batch` endpoint accepting a JSON array of `{type: "storage"|"account"|"code", addr, slot?}` requests. Returns array of results in the same order. Single DB transaction for all reads.
**Tests**: Batch of mixed storage/  account/code reads, empty batch, batch with some missing keys (partial results), large batch (1000 items), malformed items in batch.
**Depends on**: Step 7

---

## Tier 3 — Advanced Features (parallel)

### Step 9: Prefetch Endpoint — Speculative Execution
**Crate**: `crates/api` (extend)
**Scope**: `POST /v1/prefetch` endpoint. Accepts `{to, calldata, from?, value?, block_number?}`. Server executes the call with revm using an `Inspector` that records all state accesses (SLOAD, BALANCE, EXTCODEHASH, EXTCODESIZE, EXTCODECOPY). Returns `{result, access_list, state_slice}` where `state_slice` contains all accessed values. The client can use this to verify locally.
**Tests**: Prefetch a simple view function (e.g., `balanceOf`), verify result matches direct execution, verify access list contains expected slots, verify state slice is complete (client can re-execute with only the slice).
**Depends on**: Step 5, Step 7

### Step 10: WebSocket API — Interactive Slot Fetch
**Crate**: `crates/api` (extend)
**Scope**: WebSocket endpoint at `/v1/stream`. Protocol: client sends JSON messages `{type: "sload", addr, slot}`, `{type: "account", addr}`, `{type: "code", addr}`, `{type: "sload_batch", requests: [...]}`. Server responds with values. Connection-level state for tracking metrics.
**Tests**: WebSocket connection lifecycle (connect, query, disconnect), batch queries, concurrent connections, malformed messages return errors, connection timeout handling.
**Depends on**: Step 7

### Step 11: Rust Client Library — RemoteDB
**Crate**: `crates/client`
**Scope**: Implement `RemoteDB` struct that implements revm's `Database` trait but fetches state from the HTTP API (Step 7/8). Include an LRU cache for slots/accounts/code. Support both single-fetch and batch-prefetch modes. The client can call `prefetch(to, calldata)` to preload state, then execute locally.
**Tests**: Client against a mock HTTP server — verify cache hits, cache misses trigger HTTP requests, batch prefetch populates cache, LRU eviction works correctly, error handling for network failures.
**Depends on**: Step 7, Step 8

### Step 12: SQD Network Data Fetcher
**Crate**: `crates/sqd-fetcher`
**Scope**: Stream blocks + transactions from the Subsquid Network for Polygon and Ethereum. Implement pagination, error handling, and retry logic. Deserialize into the types from Step 3. Configurable block range (from/to). Output is an async stream of `Block` objects  The fetcher should use Accept-Encoding: zstd when streaming from the API.
**Tests**: Fetch a known block range (integration test, can be skipped in CI without network). Unit tests for pagination logic, retry behavior, deserialization of real SQD response fixtures. 
**Depends on**: Step 3

### Step 13: Access List Inspector for revm
**Crate**: `crates/replayer` (extend) or `crates/common`
**Scope**: Implement a revm `Inspector` that records all state accesses during EVM execution: `SLOAD` (address + slot), `BALANCE` (address), `EXTCODEHASH`/`EXTCODESIZE`/`EXTCODECOPY` (address), `SSTORE` (for detecting writes). This is the core component for the prefetch endpoint (Step 9) and client-side verification.
**Tests**: Execute known contracts, verify the inspector captures exactly the expected accesses. Test with contracts that do cross-contract calls (DELEGATECALL, STATICCALL). Test with self-destructing contracts.
**Depends on**: Step 5

---

## Tier 4 — Integration & Pipeline (parallel)

### Step 14: Full Replay Pipeline — Multi-Block
**Crate**: `crates/replayer` (extend)
**Scope**: Chain together the SQD fetcher (Step 12) and the single-block replayer (Step 6) into a continuous pipeline. Process blocks sequentially, committing state after each block. Track progress (current block, blocks/sec). Handle pipeline restarts (resume from `head_block` in metadata). Graceful shutdown.
**Tests**: Replay a range of 10-100 blocks from fixtures, verify final state matches expected values. Test restart/resume. Test error handling (corrupt block skipping, etc).
**Depends on**: Step 6, Step 12

### Step 15: Snapshot Importer
**Crate**: `crates/snapshot`
**Scope**: Import an existing state snapshot (Erigon format or custom dump) into the `StateDb`. Support reading compressed files. Progress reporting. The imported state becomes the starting point for incremental replay.
**Tests**: Import a small synthetic snapshot, verify all data is readable. Test with compressed input. Test resume after partial import.
**Depends on**: Step 2

### Step 16: State Validation Tool
**Crate**: `crates/validation`
**Scope**: CLI tool that reads N random `(address, slot)` pairs from the local `StateDb`, queries a Polygon archive RPC for the same values, and reports matches/mismatches. Support for comparing accounts, storage, and code. Binary search mode: given a diverging address+slot, find the exact block where the divergence occurred.
**Tests**: Unit tests for comparison logic, binary search logic. Integration tests against mock RPC responses.
**Depends on**: Step 2

---

## Tier 5 — Client SDK & Demo (parallel)

### Step 17: Client — Prefetch + Local Execution Flow
**Crate**: `crates/client` (extend)
**Scope**: Higher-level client API: `call(to, calldata) -> Result<Bytes>`. Internally calls the prefetch endpoint, receives the state slice, populates the local cache, then re-executes locally with revm for verification (optional). Returns the result. Configurable trust mode: `TrustServer` (use prefetch result directly) or `Verify` (re-execute locally).
**Tests**: End-to-end test against a running API server (or mock): call a view function, verify result, verify local re-execution matches server result. Test `TrustServer` mode skips local execution. Test cache is populated after prefetch.
**Depends on**: Step 9, Step 11

### Step 18: WASM Compilation of Client Core
**Crate**: `crates/client-wasm`
**Scope**: Compile the Rust client + revm into WASM using `wasm-pack`. Create JS bindings for: `createClient(endpoint)`, `call(to, calldata)`, `prefetch(to, calldata)`. Optimize binary size: `wasm-opt -Oz`, `lto = true`, `opt-level = 'z'`. Target: under 3 MB gzipped.
**Tests**: Run the WASM module in Node.js, execute a view function call against a mock server, verify the result. Measure binary size and assert it's within target.
**Depends on**: Step 17

### Step 19: TypeScript SDK Wrapper
**Crate**: N/A (npm package `@sqd/evm-state`)
**Scope**: TypeScript package wrapping the WASM core. Classes: `EvmStateClient` with methods `call(to, calldata)`, `contract(address, abi).read.functionName(args)`, `asViemTransport()`. ABI encoding/decoding via `viem` or `ethers`. The viem transport adapter intercepts `eth_call` and routes through the state service.
**Tests**: Unit tests for ABI encoding/decoding, integration test with viem `readContract()` against a mock server, test the custom transport works as a drop-in replacement.
**Depends on**: Step 18

### Step 20: Uniswap V3 Demo Application
**Crate**: `crates/demo` + TypeScript demo
**Scope**: A working example that reads Uniswap V3 pool state on Polygon: `slot0()` for current price, `liquidity()`, tick map scan. Both Rust CLI version and TypeScript version using the SDK. Demonstrates the viem drop-in replacement pattern from CONCEPT.md Section 5.5.
**Tests**: Verify the demo runs and produces valid Uniswap pool data (price within reasonable range, liquidity > 0, tick data is well-formed).
**Depends on**: Step 17 (Rust demo), Step 19 (TS demo)

---

## Tier 6 — Production Hardening (parallel)

### Step 21: Metrics & Observability
**Crate**: `crates/api` (extend)
**Scope**: Add Prometheus metrics: request count/latency by endpoint, DB read latency (p50/p95/p99), state freshness (seconds behind head), active WebSocket connections, cache hit rates. Expose at `GET /metrics`. Structured logging with `tracing`.
**Tests**: Verify metrics are emitted after API calls, verify freshness metric updates with new blocks.
**Depends on**: Step 7

### Step 22: Benchmarking Suite
**Crate**: `crates/bench`
**Scope**: Benchmarks comparing state service vs. RPC for: single call latency, 100 parallel reads throughput, 200-tick Uniswap scan. Uses `criterion` for Rust microbenchmarks and a script for end-to-end comparison. Outputs a markdown report.
**Tests**: The benchmarks themselves serve as the tests. Verify they compile and run (smoke test).
**Depends on**: Step 9, Step 11

### Step 23: Rate Limiting & Connection Management
**Crate**: `crates/api` (extend)
**Scope**: Per-IP rate limiting using `tower` middleware. Connection limits for WebSocket. Request size limits for batch endpoint. Graceful handling of slow clients (timeouts). Configurable limits via environment variables.
**Tests**: Verify rate limiting triggers after threshold, verify WebSocket connection limits, verify large batch requests are rejected, verify timeout handling.
**Depends on**: Step 7, Step 10

### Step 24: Configuration & CLI
**Crate**: `crates/cli`
**Scope**: Unified CLI binary using `clap` with subcommands: `serve` (start API server), `replay` (run the replay pipeline), `import` (import snapshot), `validate` (run validation). Configuration via TOML file and environment variables: DB path, listen address, SQD endpoint, chain spec, log level.
**Tests**: Test CLI argument parsing, test config file loading, test env var overrides.
**Depends on**: Steps 7, 14, 15, 16

---

## Dependency Graph

```
Tier 0:  [1: Workspace + Types]
              │
Tier 1:  [2: StateDB]  [3: Data Types]  [4: Chain Spec]
              │              │                 │
Tier 2:  [5: revm DB]──────[6: Replayer]  [7: HTTP API]──[8: Batch]
              │              │                 │
Tier 3:  [9: Prefetch] [13: Inspector]   [10: WebSocket] [11: Client]
         [12: SQD Fetcher]                                    │
              │              │                                │
Tier 4:  [14: Pipeline]  [15: Snapshot]  [16: Validation] [17: Prefetch Flow]
                                                              │
Tier 5:  [18: WASM]──[19: TS SDK]──[20: Demo]

Tier 6:  [21: Metrics] [22: Benchmarks] [23: Rate Limit] [24: CLI]
```

**Parallelism**: Within each tier, all steps can be done in parallel. Across tiers, you only need to wait for the specific dependency, not the whole tier.
