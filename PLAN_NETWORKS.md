# Ethereum Consensus Quirks via State-Diff Reconciliation

## Context

Ethereum mainnet has consensus-level state changes not represented as transactions:
- **Block rewards** (pre-Merge, blocks 0–15,537,393): miner gets 5/3/2 ETH + uncle rewards
- **DAO fork** (block 1,920,000): forced balance transfers from ~60 accounts
- **Beacon withdrawals** (post-Shanghai): validator withdrawal credits

SQD portal doesn't provide uncle details or withdrawals fields. But it does provide
`stateDiffs` — the authoritative final-state changes for every block, including consensus ops.

**Approach**: Replay transactions (for EVM execution correctness), then apply `stateDiffs`
on top to patch in consensus-level changes. This is universal and handles all quirks
without hardcoding each one.

## Changes

### 1. `crates/chain-spec/src/lib.rs` — Replace boolean with enum

Replace `requires_state_diffs: bool` with a pipeline strategy enum:

```rust
pub enum ReplayStrategy {
    /// Pure tx replay — no consensus quirks (test chains, simple L2s)
    Replay,
    /// Pure state-diffs — no EVM execution (fallback)
    StateDiffs,
    /// Replay txs + apply state-diffs to reconcile consensus changes
    ReplayAndReconcile,
}
```

- `polygon_mainnet()` → `ReplayAndReconcile` (replay system txs + reconcile rewards/etc)
- `ethereum_mainnet()` → `ReplayAndReconcile`
- Test helpers and benchmarks → `Replay`

### 2. `crates/sqd-fetcher/src/lib.rs` — Fetch both txs and state_diffs

Change `build_query()` to accept two independent bools: `transactions: bool, state_diffs: bool`.
When both are true, the query includes both `"transactions"` and `"stateDiffs"` selectors.

Replace `use_state_diffs: bool` field with `fetch_transactions: bool, fetch_state_diffs: bool`.
Add builder method `with_replay_and_reconcile()` that sets both to true.

### 3. `crates/replayer/src/pipeline.rs` — New pipeline mode

Add `PipelineMode::ReplayAndReconcile(ChainSpec)`:

```rust
PipelineMode::ReplayAndReconcile(chain_spec) => {
    let count = block.transactions.len();
    replay_block(db, &block, chain_spec)?;
    if !block.state_diffs.is_empty() {
        apply_state_diffs(db, &block)?;
    }
    count
}
```

This replays txs first (atomic commit), then applies state_diffs on top (second atomic commit).
State diffs overwrite only the fields they touch, using authoritative `next` values.

### 4. `crates/cli/src/main.rs` — Wire up the new strategy

Replace the `if use_state_diffs` branching with a match on `chain_spec.replay_strategy`:

```rust
match chain_spec.replay_strategy {
    ReplayStrategy::Replay => { /* txs only */ }
    ReplayStrategy::StateDiffs => { /* diffs only */ }
    ReplayStrategy::ReplayAndReconcile => { /* txs + diffs */ }
}
```

### 5. Update all `ChainSpec` construction sites

Files that construct `ChainSpec` with `requires_state_diffs`:
- `crates/chain-spec/src/lib.rs` (polygon_mainnet, ethereum_mainnet)
- `crates/replayer/src/test_helpers.rs`
- `crates/bench/src/lib.rs`
- `crates/client/src/lib.rs`
- `crates/api/src/lib.rs`

Replace `requires_state_diffs: true/false` → `replay_strategy: ReplayStrategy::*`.

## Files to modify

| File | Change |
|------|--------|
| `crates/chain-spec/src/lib.rs` | Add `ReplayStrategy` enum, replace bool field |
| `crates/sqd-fetcher/src/lib.rs` | `build_query()` supports both txs+diffs; new builder method |
| `crates/replayer/src/pipeline.rs` | Add `ReplayAndReconcile` variant |
| `crates/cli/src/main.rs` | Match on strategy instead of bool |
| `crates/replayer/src/test_helpers.rs` | Update `ChainSpec` construction |
| `crates/bench/src/lib.rs` | Update `ChainSpec` construction |
| `crates/client/src/lib.rs` | Update `ChainSpec` construction |
| `crates/api/src/lib.rs` | Update `ChainSpec` construction |

## Verification

1. `cargo build` — compilation succeeds
2. `cargo test` — all existing tests pass (test helpers use `Replay` strategy)
3. Manual: `evm-state --chain-id 1 replay --to 100` fetches blocks with both txs and state_diffs
4. Manual: check that the fetcher query JSON includes both `"transactions"` and `"stateDiffs"` selectors (visible in debug logs)
