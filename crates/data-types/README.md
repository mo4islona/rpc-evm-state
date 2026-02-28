# evm-state-data-types

Rust types for blocks, transactions, and state diffs as returned by the [SQD Network](https://docs.sqd.dev/) EVM streaming API.

## Types

### `Block`
Top-level container with a `BlockHeader` and a `Vec<Transaction>`.

### `BlockHeader`
Block metadata: `number`, `hash`, `parent_hash`, `timestamp`, `miner`, `gas_limit`, `gas_used`, `base_fee_per_gas`, `difficulty`, `mix_hash`, etc. Most fields are `Option` since presence depends on the field selector in the SQD request.

### `Transaction`
Covers legacy (type 0), EIP-2930 (type 1), and EIP-1559 (type 2) transactions. Includes both execution fields (`from`, `to`, `input`, `value`, `gas`, `nonce`) and receipt fields (`gas_used`, `cumulative_gas_used`, `status`, `contract_address`).

### `StateDiff`
A single state change within a block — storage slot modification, balance change, nonce increment, or code update. Fields: `address`, `key` (slot hash or `"balance"` / `"nonce"` / `"code"`), `kind` (`+`/`*`/`-`/`=`), `prev`, `next`.

### `HexU64`
Newtype wrapping `u64` that serializes/deserializes as a `"0x..."`-prefixed hex string, matching the SQD API convention for numeric fields like `gasLimit` and `baseFeePerGas`.

## Serde

All types use `#[serde(rename_all = "camelCase")]` to match the SQD JSON format. Optional fields use `#[serde(default)]` so missing keys deserialize as `None`.

## Test Fixtures

`fixtures/polygon_block_65000000.json` — a real Polygon PoS block with 3 transactions (EIP-1559 + legacy), fetched from the SQD Network API.
