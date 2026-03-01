# evm-state-common

Shared EVM types and encoding helpers used across the workspace.

## Types

### `AccountInfo`
Represents on-chain account state: `nonce` (u64), `balance` (U256), and `code_hash` (B256).

Provides fixed-size 72-byte binary encoding (`to_bytes` / `from_bytes`) for efficient storage in the database:
- Bytes 0..8: nonce (big-endian u64)
- Bytes 8..40: balance (big-endian U256)
- Bytes 40..72: code_hash (raw B256)

### `AccountKey`
20-byte key for the accounts table. Wraps an `Address` with `AsRef<[u8]>` / `From<&[u8]>` for direct use as a DB key.

### `StorageKey`
52-byte composite key for the storage table: `address (20B) || slot (32B)`. Constructed via `StorageKey::new(address, slot)` with accessors `address()` and `slot()`.

## Re-exports

Re-exports `Address`, `B256`, and `U256` from `alloy-primitives` for convenience.
