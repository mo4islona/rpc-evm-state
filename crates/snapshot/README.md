# evm-state-snapshot

Imports state snapshots into a [`StateDb`](../state-db). The imported state becomes the starting point for incremental replay, avoiding the need to replay the entire chain from genesis.

## Snapshot Format

JSON Lines (`.jsonl`) — one tagged JSON object per line. Supports optional zstd compression (`.jsonl.zst`).

```jsonl
{"type":"account","address":"0xd8da...","nonce":42,"balance":"0x3e8","codeHash":"0x0000..."}
{"type":"storage","address":"0xd8da...","slot":"0x0000...0001","value":"0x0000...002a"}
{"type":"code","codeHash":"0xabcd...","bytecode":"0x6060..."}
{"type":"metadata","headBlock":65000000}
```

Entry types:
- **account** — address, nonce, balance (hex U256), codeHash
- **storage** — address, slot (B256), value (hex U256)
- **code** — codeHash (B256), bytecode (hex bytes)
- **metadata** — headBlock number (set as the DB's head block on completion)

## Usage

```rust
use evm_state_snapshot::import_snapshot;
use std::path::Path;

let stats = import_snapshot(&db, Path::new("state.jsonl.zst"), |progress| {
    println!("{} entries imported, {} bytes read",
        progress.entries_imported, progress.bytes_read);
})?;

println!("Imported {} accounts, {} storage slots, {} code entries",
    stats.accounts, stats.storage_slots, stats.code_entries);
```

## Features

- **Zstd compression** — automatically detected via `.zst` file extension
- **Batched writes** — entries are flushed to the database in batches of 10,000 for efficiency
- **Resume support** — a sidecar `.progress` file tracks the number of lines imported; if the process is interrupted, re-running `import_snapshot` on the same file skips already-imported lines
- **Progress callback** — called after each batch commit with `ImportProgress { entries_imported, bytes_read }`

## Resume

If an import is interrupted, a `{file}.progress` sidecar file records how many lines were committed. On the next run, those lines are skipped. The progress file is deleted on successful completion.
