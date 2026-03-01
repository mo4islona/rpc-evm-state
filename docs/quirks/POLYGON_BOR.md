# Polygon PoS (Bor) — Chain-Specific Quirks

## Architecture Overview

Polygon PoS uses a dual-consensus architecture: **Heimdall** (Tendermint-based) for
checkpointing and validator management on Ethereum L1, and **Bor** (fork of Geth) for
block production on L2.

```
 Ethereum L1                       Polygon L2 (Bor)
┌──────────────┐                  ┌───────────────────────────────────┐
│              │  checkpoints     │                                   │
│  Heimdall    │◄─────────────────│   Bor block production            │
│  validators  │                  │                                   │
│              │  state sync      │   ┌─────────┐   ┌─────────────┐  │
│  StateSender ├─────────────────►│   │ Normal   │ + │ State Sync  │  │
│  contract    │  events          │   │ txs      │   │ txs (0x0)   │  │
│              │                  │   └─────────┘   └─────────────┘  │
└──────────────┘                  └───────────────────────────────────┘
```

## State Sync Transactions

When users bridge assets from Ethereum L1 to Polygon L2, the flow is:

1. User calls `StateSender.syncState()` on Ethereum L1
2. Heimdall validators observe the event and reach consensus
3. Bor injects a **state sync transaction** at the end of a sprint boundary block
4. The `StateReceiver` contract (at `0x0000000000000000000000000000000000001001`)
   processes the state sync and credits the user

These state sync transactions are **not real EVM transactions** — they are consensus-level
injections that appear in the block but bypass normal transaction validation.

### How to recognize them

```
from:      0x0000000000000000000000000000000000000000  (zero address)
gas:       0x0                                         (zero gas limit)
to:        0x0000000000000000000000000000000000001001  (StateReceiver)
gasPrice:  0x0
nonce:     (arbitrary — not validated)
```

### Normal tx vs State Sync tx

```
                 Normal Transaction              State Sync Transaction
                ┌──────────────────┐            ┌──────────────────────┐
  from          │ EOA address      │            │ 0x0 (zero address)   │
  gas           │ > 0              │            │ 0                    │
  gasPrice      │ > 0 (usually)    │            │ 0                    │
  nonce check   │ yes              │            │ NO                   │
  balance check │ yes              │            │ NO                   │
  nonce bump    │ yes (+1)         │            │ NO                   │
  gas deduction │ yes              │            │ NO                   │
  EVM execution │ full pipeline    │            │ execution only       │
                └──────────────────┘            └──────────────────────┘
```

## How We Handle It

### Detection (`crates/replayer/src/lib.rs:163-167`)

```rust
fn is_system_tx(tx: &Transaction) -> bool {
    let from_zero = tx.from.map_or(false, |a| a.is_zero());
    let gas_zero  = tx.gas.map_or(true, |g| g.as_u64() == 0);
    from_zero && gas_zero
}
```

Both conditions must be true: sender is zero address **and** gas limit is zero.

### Execution Path

```
                    ┌─────────────┐
                    │  for each   │
                    │ transaction │
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │ is_system?  │
                    └──┬──────┬───┘
                   yes │      │ no
              ┌────────▼──┐ ┌─▼────────────┐
              │ Adjust:   │ │              │
              │ gas_limit │ │ evm.replay() │
              │  = block  │ │ (full EVM    │
              │ gas_price │ │  pipeline)   │
              │  = 0      │ │              │
              └────┬──────┘ └──────┬───────┘
                   │               │
          ┌────────▼────────┐      │
          │ handler          │      │
          │ .run_system_call │      │
          │                  │      │
          │ Skips:           │      │
          │ - nonce check    │      │
          │ - balance check  │      │
          │ - nonce bump     │      │
          │ - gas deduction  │      │
          │                  │      │
          │ Runs:            │      │
          │ - EVM execution  │      │
          │ - state output   │      │
          └────────┬─────────┘      │
                   │                │
                   └───────┬────────┘
                           │
                    ┌──────▼──────┐
                    │cache_db     │
                    │ .commit()   │
                    └──────┬──────┘
                           │
                    ┌──────▼──────────┐
                    │flush_cache_to_db│
                    │ (atomic batch)  │
                    └─────────────────┘
```

### Pre-execution adjustment (`crates/replayer/src/lib.rs:67-73`)

```rust
if is_system {
    tx_env.gas_limit = block_env.gas_limit;  // use block gas limit
    tx_env.gas_price = 0;                     // no gas cost
}
```

The gas limit is set to the block's gas limit because the consensus layer manages the real
gas accounting. Setting gas_price to 0 ensures no balance is deducted from the zero address.

### System call execution (`crates/replayer/src/lib.rs:113-122`)

```rust
if is_system {
    let mut handler = MainnetHandler::<_, _, EthFrame<_, _, _>>::default();
    handler.run_system_call(&mut evm)
} else {
    evm.replay()
}
```

`run_system_call` from revm runs **only** the execution and output phases:
- No `validate_tx` (nonce, balance, gas checks)
- No `pre_execution` (nonce increment, gas deduction)
- This matches how the Bor client applies state sync internally

## Hardfork Schedule

Polygon uses **block numbers** for all hardfork activations, including post-Merge forks.
There is no "Merge" because Polygon never used Proof of Work — it uses Bor/Heimdall
consensus from genesis.

```
Block 0           Petersburg (genesis)
Block 3,395,000   Istanbul / MuirGlacier
Block 14,750,000  Berlin
Block 23,850,000  London (= Bor "Jaipur")
Block 50,523,000  Shanghai
Block 54,876,000  Cancun (= Bor "Napoli")
Block 73,440,256  Prague (= Bor "Bhilai")
```

Source: `crates/chain-spec/src/lib.rs:69-115`

Note: MuirGlacier only affects the difficulty bomb — since Bor has no PoW, it has no
practical effect, but we match the official config for correctness.

### Ethereum vs Polygon activation comparison

```
                   Ethereum                Polygon
Activation type    block → timestamp       block only (always)
                   (after Merge)
Merge/PoS          block 15,537,394        N/A (PoS from genesis)
Shanghai           ts 1,681,338,455        block 50,523,000
Cancun             ts 1,710,338,135        block 54,876,000
Prague             ts 1,746,612,311        block 73,440,256
```

## Data Source: SQD Network

State sync transactions are included in the SQD Network's `polygon-mainnet` dataset
as regular transactions in the block's transaction list. The detection is purely based
on the `from` and `gas` fields — no extra RPC calls or chain-specific SQD fields are
needed.

```
SQD Portal ──NDJSON──► Block { transactions: [
                            Tx { from: 0xABC.., gas: 21000, .. },   ← normal
                            Tx { from: 0xDEF.., gas: 50000, .. },   ← normal
                            Tx { from: 0x000.., gas: 0,     .. },   ← state sync!
                        ]}
                              │
                              ▼
                        is_system_tx() → run_system_call()
```

## Known Edge Cases

### 1. Sprint boundary blocks

State sync transactions appear only at the end of sprint boundary blocks (every 16 or 64
blocks depending on the fork). A single block can contain multiple state sync txs.

### 2. `bor_getAuthor` / `bor_getAuthor`

The Bor RPC exposes `bor_getAuthor` to get the block producer. This is **not** needed
for replay because the block header's `miner` field (mapped to `beneficiary` in revm's
`BlockEnv`) already carries this information in the SQD data.

### 3. Zero-address miner

On Polygon, the `miner` field in the header is often `0x0000...0000`. This is expected —
Bor uses a separate mechanism for block rewards that doesn't rely on the coinbase field
in the same way as Ethereum PoW did.

### 4. Transaction ordering

State sync txs are always the **last** transactions in a block. The SQD dataset preserves
this ordering via `transactionIndex`, which the pipeline processes sequentially.

### 5. Gas used by state sync

State sync transactions do consume gas during execution (the `StateReceiver` contract
runs real EVM code), but this gas is not charged to any account. The `gas_used` in the
receipt reflects actual computation, even though `gas_price = 0`.

## Files Reference

| File | Role |
|------|------|
| `crates/replayer/src/lib.rs:158-167` | `is_system_tx()` detection |
| `crates/replayer/src/lib.rs:67-73` | Gas limit/price adjustment |
| `crates/replayer/src/lib.rs:113-122` | `run_system_call()` execution path |
| `crates/chain-spec/src/lib.rs:69-115` | Polygon hardfork schedule |
| `crates/data-types/src/lib.rs:89-153` | Transaction struct with `from`/`gas` fields |
| `genesis.json` | Polygon mainnet genesis (Bor format) |
