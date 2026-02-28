# evm-state-demo

Uniswap V3 pool reader that demonstrates reading on-chain state via the EVM state service instead of an RPC node.

Reads the WMATIC/USDC 0.05% pool on Polygon:
- `slot0()` — current price (sqrtPriceX96) and tick
- `liquidity()` — active liquidity in the current range
- `tickSpacing()` — tick spacing for the fee tier
- `ticks(i)` — scan 200+ ticks around the current price

## Prerequisites

A running EVM state API server with indexed Polygon state.

## Usage

```bash
# Default pool (WMATIC/USDC 0.05%)
cargo run -p evm-state-demo -- http://localhost:3000

# Custom pool address
cargo run -p evm-state-demo -- http://localhost:3000 0xa374094527e1673a86de625aa7d246023b9e3d71
```

## How it works

Uses `alloy-sol-types` `sol!` macro for type-safe ABI encoding/decoding and `evm-state-client::StateClient` for executing calls via the prefetch endpoint. Each call is a single HTTP round-trip — no full node required.

See `packages/demo/` for the TypeScript version using the viem drop-in transport.
