# evm-state-chain-spec

Chain specification and hardfork configuration for EVM replay. Maps block numbers and timestamps to revm `SpecId` values and provides helpers to build revm execution environments from SQD data types.

## Multi-network Support

Hardfork activations can be block-based or timestamp-based via `HardforkCondition`:

- `HardforkCondition::Block(n)` — activates at block number `n` (used by all pre-Merge forks and Polygon)
- `HardforkCondition::Timestamp(t)` — activates at Unix timestamp `t` (used by Ethereum post-Merge forks)

Use `ChainSpec::spec_at(block_number, timestamp)` to get the active `SpecId`.

Use `from_chain_id(id)` to look up a built-in spec by chain ID.

## Chain Specs

### `ethereum_mainnet()` — chain ID 1

Sourced from [go-ethereum's config.go](https://github.com/ethereum/go-ethereum/blob/master/params/config.go):

| Condition | Value | Hardfork |
|---|---|---|
| Block | 0 | Frontier |
| Block | 200,000 | Frontier Thawing |
| Block | 1,150,000 | Homestead |
| Block | 1,920,000 | DAO Fork |
| Block | 2,463,000 | Tangerine Whistle |
| Block | 2,675,000 | Spurious Dragon |
| Block | 4,370,000 | Byzantium |
| Block | 7,280,000 | Petersburg |
| Block | 9,069,000 | Istanbul |
| Block | 9,200,000 | Muir Glacier |
| Block | 12,244,000 | Berlin |
| Block | 12,965,000 | London |
| Block | 13,773,000 | Arrow Glacier |
| Block | 15,050,000 | Gray Glacier |
| Block | 15,537,394 | Merge (Paris) |
| Timestamp | 1,681,338,455 | Shanghai |
| Timestamp | 1,710,338,135 | Cancun |
| Timestamp | 1,746,612,311 | Prague |

### `polygon_mainnet()` — chain ID 137

Sourced from [Bor's config.go](https://github.com/maticnetwork/bor/blob/develop/params/config.go). All activations are block-based (Polygon uses Bor consensus, no Merge):

| Block | Hardfork |
|---|---|
| 0 | Petersburg |
| 3,395,000 | Istanbul / MuirGlacier |
| 14,750,000 | Berlin |
| 23,850,000 | London |
| 50,523,000 | Shanghai |
| 54,876,000 | Cancun |
| 73,440,256 | Prague |

## Environment Builders

### `block_env_from_header(header) -> BlockEnv`
Converts a SQD `BlockHeader` into a revm `BlockEnv` (block number, beneficiary, timestamp, gas limit, base fee, difficulty, prevrandao).

### `tx_env_from_transaction(tx, chain_id) -> TxEnv`
Converts a SQD `Transaction` into a revm `TxEnv`. Handles both legacy (gas_price) and EIP-1559 (max_fee_per_gas / max_priority_fee_per_gas) transactions. Sets `TxKind::Call` or `TxKind::Create` based on the `to` field.
