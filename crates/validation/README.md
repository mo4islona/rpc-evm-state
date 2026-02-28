# evm-state-validation

Validates local [`StateDb`](../state-db) contents against a remote Ethereum/Polygon archive RPC node. Detects replay bugs by sampling random state entries and comparing them against the canonical chain.

## Features

- **Random sampling** — picks N random accounts and storage slots from the local DB, queries the remote RPC at a given block, and reports matches/mismatches
- **Binary search** — given a diverging `(address, slot)`, finds the exact block where the remote value started differing from the local value
- **Sync JSON-RPC client** — minimal `EthRpcClient` using `ureq` for `eth_getBalance`, `eth_getTransactionCount`, `eth_getCode`, `eth_getStorageAt`

## Usage

```rust
use evm_state_validation::{EthRpcClient, validate_random, find_divergence_block};
use evm_state_db::StateDb;

let db = StateDb::open("path/to/db")?;
let rpc = EthRpcClient::new("https://polygon-rpc.com");

let head = db.get_head_block()?.unwrap();

// Sample 100 random accounts and 100 storage slots, compare against RPC
let report = validate_random(&db, &rpc, 100, head)?;
println!("{} checks, {} mismatches", report.total_checks(), report.mismatches());

if !report.is_valid() {
    // Find exactly which block introduced the first storage mismatch
    let mismatch = &report.storage_checks.iter().find(|c| !c.matches).unwrap();
    let block = find_divergence_block(
        &rpc,
        &mismatch.address,
        &mismatch.slot,
        mismatch.local,
        1,    // low block (e.g. snapshot block)
        head, // high block
    )?;
    println!("divergence at block {:?}", block);
}
```

## Validation report

`ValidationReport` contains:
- `account_checks` — one `AccountCheck` per field (nonce, balance, codeHash) per sampled account
- `storage_checks` — one `StorageCheck` per sampled storage slot
- `is_valid()` — true if all checks match
- `mismatches()` — count of failed checks

## Binary search

`find_divergence_block()` performs a binary search over block numbers using the remote RPC. It queries `eth_getStorageAt` at various blocks to find the first block where the remote value diverges from the local value. Returns `None` if no divergence exists at the high block.
