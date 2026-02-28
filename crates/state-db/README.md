# evm-state-db

Flat key-value state database backed by [libmdbx](https://github.com/erthink/libmdbx). Stores EVM account state, storage slots, contract bytecode, and sync metadata.

## Tables

| Table | Key | Value |
|---|---|---|
| `accounts` | `Address` (20B) | `AccountInfo` (72B binary) |
| `storage` | `Address \|\| Slot` (52B) | `U256` (32B big-endian) |
| `code` | `B256` code hash (32B) | Raw bytecode |
| `metadata` | UTF-8 string | Arbitrary bytes |

## Usage

```rust
let db = StateDb::open(path)?;

// Read/write accounts
db.set_account(addr, &info)?;
let info = db.get_account(addr)?;

// Read/write storage slots
db.set_storage(addr, slot, value)?;
let val = db.get_storage(addr, slot)?;

// Read/write contract code
db.set_code(code_hash, &bytecode)?;
let code = db.get_code(code_hash)?;

// Track sync progress
db.set_head_block(65_000_000)?;
let head = db.get_head_block()?;
```

## Batch Writes

`WriteBatch` groups multiple writes into a single atomic libmdbx transaction. If not explicitly committed, the batch rolls back on drop.

```rust
let mut batch = db.write_batch()?;
batch.set_account(addr, &info)?;
batch.set_storage(addr, slot, value)?;
batch.commit()?;
```

## Design Choices

- **No Merkle trie** — flat KV layout optimized for point reads (sub-microsecond via mmap).
- **Big-endian encoding** — preserves lexicographic ordering for potential range scans.
- **Generic over transaction kind** — internal helpers work with both RO and RW libmdbx transactions.
