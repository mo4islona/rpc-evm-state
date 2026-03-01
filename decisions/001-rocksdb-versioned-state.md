# ADR-001: RocksDB for Versioned State Database

**Status:** Accepted
**Date:** 2026-02-28
**Context:** Migrating from libmdbx (latest-state-only) to a versioned state database supporting historical queries at any block height.

## Problem

The current `StateDb` uses libmdbx with a flat key schema that stores only the HEAD state. To support historical access ("state at block N") we need a storage engine that efficiently handles:

1. **Temporal key encoding** — appending an inverted block height to every key, so a prefix seek returns the most recent version first (the Erigon/Reth pattern).
2. **Append-heavy write pattern** — each block appends new key versions without modifying existing entries. Over millions of blocks this creates billions of entries.
3. **Storage efficiency** — Polygon full history is estimated at 200-500 GB compressed; Ethereum at 2-5 TB. Compression is critical.
4. **Concurrent read/write** — the replay pipeline writes blocks continuously while the HTTP API serves reads. The engine must not block readers during writes.

## Options Considered

### 1. Keep libmdbx (current)

libmdbx is an LMDB fork optimized for read-heavy workloads with MVCC readers.

**Pros:**
- Already integrated, zero migration cost
- Excellent read latency (memory-mapped I/O)
- ACID transactions with zero-copy reads

**Cons:**
- **No compression** — data stored at full size on disk. For versioned state with billions of entries, this is prohibitive. Polygon history would be 1-2 TB uncompressed vs 200-500 GB with zstd.
- **Write lock blocks all readers** — a single writer transaction blocks all new read transactions until it commits. During block replay (continuous writes), API read latency spikes. This is documented as a known issue in CONCEPT.md section 6.7.
- **No column families** — cannot apply different compression/compaction strategies to accounts vs storage vs code.
- **No bloom filters** — every read for a non-existent key requires a full B-tree traversal. In EVM, most `SLOAD` calls return zero (non-existent slot), making this the dominant read pattern.
- **Memory-mapped size limits** — the database must fit in the configured memory map size. Growing beyond requires reconfiguration and restart.
- **No prefix bloom filters** — the inverted-block-height key scheme relies on efficient prefix iteration. Without prefix-aware filtering, every prefix seek touches disk.

### 2. RocksDB (chosen)

RocksDB is a persistent key-value store optimized for fast storage, developed by Facebook/Meta. Used in production by Ethereum clients (Erigon, Reth), CockroachDB, TiKV, and many blockchain projects.

**Pros:**
- **Zstd compression** — built-in, per-column-family. Typically achieves 3-5x compression on EVM state data. This is the single biggest factor for storage efficiency.
- **Column families** — separate `accounts`, `storage`, `code`, `metadata` into independent column families with different compression, compaction, and bloom filter settings. Storage CF gets universal compaction (optimized for append-heavy); code CF gets zstd dictionary training (bytecode compresses well with shared dictionaries).
- **Bloom filters** — 10-bit bloom filters eliminate >99% of disk reads for non-existent keys. Critical for EVM where most storage slot reads return zero.
- **Prefix extractors + prefix bloom filters** — native support for the `(address)` and `(address, slot)` prefix seek pattern. Combined with bloom filters, this makes historical lookups fast without touching irrelevant key ranges.
- **Concurrent read/write** — readers never block writers and vice versa. The replay pipeline and API server operate independently.
- **Universal compaction** — ideal for the append-only historical state pattern. Reduces write amplification compared to leveled compaction.
- **Battle-tested for this exact use case** — Erigon and Reth use RocksDB with the same inverted-block-height key scheme we're implementing. The approach is proven at Ethereum mainnet scale.
- **Tunable** — extensive knobs for block cache size, write buffer size, compaction triggers, compression levels, allowing optimization as the dataset grows.

**Cons:**
- **Higher read latency than libmdbx** — RocksDB reads go through the block cache rather than direct memory mapping. Typical read latency is ~10-50us vs ~1-5us for libmdbx. However, this is irrelevant for our use case: even 50us is 1000x faster than an RPC round-trip (~50ms).
- **Write amplification** — LSM-tree compaction rewrites data multiple times. Mitigated by universal compaction for append-only data and acceptable given our write pattern (sequential block processing, not random writes).
- **Larger dependency** — RocksDB is a C++ library linked via FFI. Increases build time and binary size. The `rocksdb` Rust crate handles the binding.
- **No zero-copy reads** — values are copied into Rust memory on read, unlike libmdbx's zero-copy slices. Negligible overhead for our value sizes (32-72 bytes).

### 3. LMDB

The original database that libmdbx forked from.

**Pros:**
- Similar API to our current libmdbx integration
- Mature and stable

**Cons:**
- Same limitations as libmdbx (no compression, no bloom filters, no column families, write lock blocks readers)
- Strictly worse than libmdbx for our use case — libmdbx adds improvements over LMDB that we already benefit from
- No advantage over keeping libmdbx

**Rejected:** all of libmdbx's cons apply, with no compensating advantages.

### 4. SQLite

General-purpose embedded SQL database.

**Pros:**
- Simple SQL interface
- Single-file storage
- Excellent tooling and debugging

**Cons:**
- **Not designed for this access pattern** — our workload is pure key-value with prefix iteration. SQL parsing overhead is pure waste.
- **No column families or per-table compression**
- **Row-level locking overhead** — unnecessary for our append-only write pattern
- **No prefix bloom filters**
- **Significantly slower** for our access pattern — benchmarks consistently show 5-10x slower than RocksDB for sorted key-value workloads

**Rejected:** wrong tool for a sorted key-value workload.

## Decision

**Use RocksDB** via the [`rocksdb`](https://crates.io/crates/rocksdb) Rust crate (v0.22+, `zstd` feature enabled).

The primary drivers are:

1. **Zstd compression** — reduces storage 3-5x, making full chain history feasible on commodity hardware
2. **Bloom filters + prefix extractors** — eliminate disk reads for the dominant "non-existent key" access pattern in EVM
3. **Column families** — enable per-table optimization (universal compaction for append-heavy storage, dictionary-trained zstd for bytecode)
4. **Concurrent read/write** — removes the API latency spikes caused by libmdbx's write lock
5. **Proven at scale** — Erigon and Reth validate this exact approach (RocksDB + inverted block height) on Ethereum mainnet

The read latency tradeoff (10-50us vs 1-5us) is acceptable because our reads are already 1000x faster than the RPC calls they replace.

## Consequences

- All `StateDb` internals rewritten (backend swap is isolated to `crates/state-db`)
- `WriteBatch` methods change from `&self` to `&mut self` — ripples to all callers
- Build time increases due to RocksDB C++ compilation (~30s additional on first build)
- Existing `.mdbx` databases are not compatible — requires re-replay or snapshot re-import
- Default database path changes from `./state.mdbx` to `./state.rocksdb`
