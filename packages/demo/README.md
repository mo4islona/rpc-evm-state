# @sqd/evm-state-demo

Uniswap V3 pool reader demonstrating the `@sqd/evm-state` SDK as a viem drop-in replacement.

Reads the WMATIC/USDC 0.05% pool on Polygon using standard viem `readContract` calls routed through the EVM state service — no RPC node, no API keys, no rate limits.

## Prerequisites

- A running EVM state API server with indexed Polygon state
- The WASM package built (`wasm-pack build crates/client-wasm --target web`)

## Install

```bash
cd packages/demo
npm install
```

## Usage

```bash
# Run the demo (default endpoint: http://localhost:3000)
npm run demo

# Custom endpoint
npm run demo -- http://localhost:3000

# Run viem transport demo only
npm run demo:viem -- http://localhost:3000
```

## What it demonstrates

The key code pattern — existing viem code works unchanged:

```typescript
import { createPublicClient } from 'viem';
import { polygon } from 'viem/chains';
import { EvmStateClient } from '@sqd/evm-state';

const sqd = new EvmStateClient({ endpoint: 'http://localhost:3000' });
const client = createPublicClient({
  chain: polygon,
  transport: sqd.asViemTransport(),
});

// Standard viem code — no changes needed
const slot0 = await client.readContract({
  address: poolAddress,
  abi: uniswapV3PoolAbi,
  functionName: 'slot0',
});
```

See `crates/demo/` for the Rust version using `alloy-sol-types`.
