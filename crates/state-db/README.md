# evm-state-db

Flat key-value state database backed by [RocksDB](https://github.com/facebook/rocksdb). Stores EVM account state, storage slots, contract bytecode, and sync metadata.

## Column Families

| Column Family | Key | Value |
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

## Iteration

Enumeration of all entries in a column family:

```rust
let accounts: Vec<(Address, AccountInfo)> = db.iter_accounts()?;
let storage: Vec<(Address, B256, U256)> = db.iter_storage()?;
```

Useful for validation, export, and sampling workflows.

## Batch Writes

`WriteBatch` groups multiple writes into a single atomic RocksDB write batch. If not explicitly committed, the batch is discarded on drop.

```rust
let mut batch = db.write_batch()?;
batch.set_account(addr, &info)?;
batch.set_storage(addr, slot, value)?;
batch.commit()?;
```

## revm Database Integration

`StateDb` implements both `revm::Database` and `revm::DatabaseRef` traits, allowing it to be used directly as the backing store for EVM execution:

- `basic(address)` → reads account info from the `accounts` column family
- `code_by_hash(hash)` → reads bytecode from the `code` column family
- `storage(address, index)` → reads slot values from the `storage` column family
- `block_hash(number)` → returns `B256::ZERO` (block hashes not stored)

This means you can pass `&mut StateDb` directly to revm's `Context::new()` and execute EVM transactions against the stored state.

## Design Choices

- **No Merkle trie** — flat KV layout optimized for point reads.
- **Big-endian encoding** — preserves lexicographic ordering for range scans and prefix iteration.
- **Column families** — separate accounts, storage, code, and metadata with independent configuration (compression, bloom filters, compaction strategy).
