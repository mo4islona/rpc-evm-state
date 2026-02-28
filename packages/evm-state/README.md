# @sqd/evm-state

TypeScript SDK for the EVM state service. Provides three abstraction levels for executing read-only EVM calls against pre-indexed state.

## Install

```bash
npm install @sqd/evm-state
```

## Usage

### 1. Raw calls

Execute low-level EVM calls with hex-encoded calldata:

```ts
import { EvmStateClient } from '@sqd/evm-state';

const client = new EvmStateClient({ endpoint: 'http://localhost:3000' });

const result = await client.call(
  '0xContractAddress',
  '0xEncodedCalldata',
  { from: '0xCaller', value: 0n },
);

console.log(result.output);   // "0x..."
console.log(result.success);  // true/false
console.log(result.verified); // null | true | false
```

### 2. ABI-aware contract reads

Encode and decode calls automatically using a viem-compatible ABI:

```ts
import { EvmStateClient } from '@sqd/evm-state';
import { parseAbi } from 'viem';

const client = new EvmStateClient({ endpoint: 'http://localhost:3000' });

const erc20Abi = parseAbi([
  'function balanceOf(address owner) view returns (uint256)',
  'function decimals() view returns (uint8)',
  'function totalSupply() view returns (uint256)',
]);

const erc20 = client.contract('0xTokenAddress', erc20Abi);

const balance = await erc20.read.balanceOf(['0xOwnerAddress']);
const decimals = await erc20.read.decimals();
```

### 3. Viem drop-in transport

Use with viem's `createPublicClient` for zero-change integration with existing code:

```ts
import { createPublicClient } from 'viem';
import { polygon } from 'viem/chains';
import { EvmStateClient } from '@sqd/evm-state';

const sqd = new EvmStateClient({ endpoint: 'http://localhost:3000' });

const publicClient = createPublicClient({
  chain: polygon,
  transport: sqd.asViemTransport(),
});

// readContract now uses the state service instead of an RPC node
const balance = await publicClient.readContract({
  address: '0xTokenAddress',
  abi: erc20Abi,
  functionName: 'balanceOf',
  args: ['0xOwnerAddress'],
});
```

## Verification mode

Enable local re-execution with revm to verify server responses:

```ts
const client = new EvmStateClient({
  endpoint: 'http://localhost:3000',
  verify: true,
});

const result = await client.call('0xContract', '0xCalldata');
// result.verified === true  → local execution matches server
// result.verified === false → divergence detected
// result.verified === null  → verification not enabled
```

## API

### `EvmStateClient`

| Method | Description |
|--------|-------------|
| `new EvmStateClient(config)` | Create a client with `{ endpoint, verify? }` |
| `call(to, calldata, options?)` | Execute a raw EVM call |
| `contract(address, abi)` | Create an ABI-aware contract reader |
| `asViemTransport()` | Create a viem-compatible custom transport |

### `CallResult`

| Field | Type | Description |
|-------|------|-------------|
| `output` | `` `0x${string}` `` | Raw output bytes as hex |
| `success` | `boolean` | Whether the EVM call succeeded |
| `verified` | `boolean \| null` | Verification result |

## Supported RPC methods

The viem transport intercepts `eth_call` only. All other RPC methods will throw. This is by design — the state service provides read-only contract state, not a full Ethereum node.
