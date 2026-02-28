# evm-state-bench

Benchmarking suite for the EVM state service. Compares the state service (prefetch + local read) against traditional JSON-RPC for common access patterns.

## What it benchmarks

| Scenario | Description |
|----------|-------------|
| **Single call latency** | One `eth_call` via the prefetch endpoint (TrustServer + Verify modes) |
| **100 parallel reads** | 100 sequential `eth_getStorageAt` equivalent reads via RemoteDB |
| **200-tick Uniswap scan** | Simulated Uniswap V3 tick bitmap scan — single prefetch reading 200 storage slots vs. 200 individual RPC calls |

## Running criterion benchmarks

```bash
cargo bench -p evm-state-bench
```

HTML reports are generated in `target/criterion/`.

## Running the markdown report

```bash
cargo run --release -p evm-state-bench --bin report [iterations]
```

Default is 100 iterations. To compare against an external RPC:

```bash
RPC_URL=https://polygon-rpc.com cargo run --release -p evm-state-bench --bin report
```

## How it works

The benchmarks spin up a local API server backed by an in-memory libmdbx database pre-populated with synthetic state. The `StateClient` and `RemoteDB` from `evm-state-client` are used to make requests, measuring round-trip latency.

When `RPC_URL` is set, the same operations are also performed against the external RPC for a head-to-head comparison.
