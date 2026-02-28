# evm-state-replayer

Replays EVM blocks against a [`StateDb`](../state-db) to reconstruct state. Executes all transactions in order using [revm](https://github.com/bluealloy/revm) and commits the resulting state changes atomically.

## How it works

1. A `CacheDB` overlay wraps the `StateDb` as a read-only backing store.
2. Each transaction gets a fresh EVM context (clean journal), but the `CacheDB` persists across transactions so state changes accumulate within the block.
3. After all transactions execute, the `CacheDB` cache is flushed to `StateDb` in a single atomic `WriteBatch`.

This ensures that:
- Transaction N can see state changes from transactions 0..N-1 (intra-block visibility).
- Either all changes from a block are committed, or none are (atomicity).
- The head block number is updated as part of the same batch.

## Usage

```rust
use evm_state_replayer::replay_block;

let result = replay_block(&db, &block, &chain_spec)?;

for (i, tx) in result.tx_results.iter().enumerate() {
    println!("tx {i}: gas_used={}, success={}", tx.gas_used, tx.success);
}
// State changes are already committed to db at this point.
```

## Error handling

- Database errors (libmdbx failures) propagate as `Error::Db`.
- EVM execution errors (invalid header, missing fields) propagate as `Error::Evm` with the failing transaction index.
- Reverted transactions are **not** errors — they produce `TxResult { success: false }` and their gas is consumed, but state changes from the revert are rolled back by revm.
