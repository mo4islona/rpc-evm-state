# evm-state-client-wasm

WASM-compiled EVM state client with JavaScript bindings. Wraps revm and the state API prefetch protocol into a `~800 KB` gzipped WASM module that runs in browsers, workers, and Node.js 18+.

## Features

- **`WasmStateClient`** — async `call()` and `prefetch()` methods callable from JavaScript
- **Local revm verification** — optionally re-executes calls locally using state pre-warmed by the server
- **Pure-Rust crypto** — uses `k256` + `kzg-rs` (no C/asm), compiles cleanly to `wasm32-unknown-unknown`
- **Zero-copy cache** — `RefCell<HashMap>` cache populated by prefetch, read synchronously by revm

## Build

```bash
wasm-pack build crates/client-wasm --target web --release
```

The output is in `crates/client-wasm/pkg/`:
- `evm_state_client_wasm_bg.wasm` — the WASM binary
- `evm_state_client_wasm.js` — JS glue code
- `evm_state_client_wasm.d.ts` — TypeScript type definitions
- `package.json` — npm package manifest

## JavaScript API

```javascript
import init, { WasmStateClient, createClient } from './pkg/evm_state_client_wasm.js';

await init();

// Create a client (verify=false for TrustServer mode)
const client = new WasmStateClient("http://localhost:3000", false);
// or: const client = createClient("http://localhost:3000", false);

// Execute an EVM call
const result = await client.call(
  "0xContractAddress",
  "0xCalldata",
  "0xFromAddress",  // optional
  null              // value, optional
);
console.log(result.output);   // "0x..."
console.log(result.success);  // true/false
console.log(result.verified); // null (TrustServer) or true/false (Verify)

// Prefetch only (populate cache without local execution)
const prefetch = await client.prefetch("0xContractAddress", "0xCalldata");
console.log(prefetch.result);  // "0x..."
console.log(prefetch.success); // true/false
```

### Verify mode

Pass `verify=true` to re-execute calls locally with revm:

```javascript
const client = new WasmStateClient("http://localhost:3000", true);
const result = await client.call("0xContract", "0xCalldata");
// result.verified === true  → local execution matches server
// result.verified === false → divergence detected
```

## Binary size

| Metric | Size |
|--------|------|
| Raw WASM | ~1.3 MB |
| Gzipped | ~800 KB |

Optimized with `opt-level = 'z'`, LTO, single codegen unit, and symbol stripping.

## How it works

1. `call()` sends a POST to `/v1/prefetch` — the server executes the call and returns the result plus all accessed state (accounts, storage, code)
2. The state slice is loaded into an in-memory `HashMap` cache
3. If verify mode is enabled, revm re-executes the call locally reading from the warm cache (no additional HTTP calls)
4. Returns the result to JavaScript as a plain object
