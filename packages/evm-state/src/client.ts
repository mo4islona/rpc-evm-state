import type { Abi } from "viem";
import { createContractReader, type ContractReader } from "./contract.js";
import { createEvmStateTransport } from "./viem-transport.js";

export interface EvmStateClientConfig {
  /** URL of the EVM state API server. */
  endpoint: string;
  /** If true, calls are re-executed locally with revm for verification. */
  verify?: boolean;
}

export interface CallResult {
  /** Raw output bytes as hex. */
  output: `0x${string}`;
  /** Whether the EVM call succeeded. */
  success: boolean;
  /** Verification result: true if local matches server, false if diverged, null if not verified. */
  verified: boolean | null;
}

export interface CallOptions {
  from?: `0x${string}`;
  value?: bigint;
}

/**
 * Internal interface matching the WASM client's API.
 * This allows mocking in tests without loading the WASM module.
 */
export interface WasmClient {
  call(
    to: string,
    calldata: string,
    from?: string | null,
    value?: string | null,
  ): Promise<{ output: string; success: boolean; verified: boolean | null }>;
  prefetch(
    to: string,
    calldata: string,
  ): Promise<{ result: string; success: boolean }>;
}

/**
 * High-level EVM state client.
 *
 * Provides three abstraction levels:
 * 1. **Raw calls**: `client.call(to, calldata)`
 * 2. **ABI-aware**: `client.contract(address, abi).read.functionName(args)`
 * 3. **Viem drop-in**: `client.asViemTransport()` for use with `createPublicClient`
 */
export class EvmStateClient {
  private readonly config: EvmStateClientConfig;
  private wasmClient: WasmClient | null = null;
  private initPromise: Promise<void> | null = null;

  constructor(config: EvmStateClientConfig) {
    this.config = config;
  }

  /**
   * Create an EvmStateClient with an already-initialized WASM client.
   * Useful for testing or when you've already loaded the WASM module.
   */
  static fromWasmClient(
    wasm: WasmClient,
    config?: Partial<EvmStateClientConfig>,
  ): EvmStateClient {
    const client = new EvmStateClient({
      endpoint: config?.endpoint ?? "mock://",
      verify: config?.verify,
    });
    client.wasmClient = wasm;
    return client;
  }

  private async ensureInitialized(): Promise<WasmClient> {
    if (this.wasmClient) return this.wasmClient;

    if (!this.initPromise) {
      this.initPromise = (async () => {
        const wasm = await import("evm-state-client-wasm");
        await wasm.default();
        this.wasmClient = new wasm.WasmStateClient(
          this.config.endpoint,
          this.config.verify ?? false,
        );
      })();
    }

    await this.initPromise;
    return this.wasmClient!;
  }

  /**
   * Execute a raw EVM call.
   *
   * @param to - Target contract address
   * @param calldata - ABI-encoded calldata
   * @param options - Optional from address and value
   */
  async call(
    to: `0x${string}`,
    calldata: `0x${string}`,
    options?: CallOptions,
  ): Promise<CallResult> {
    const wasm = await this.ensureInitialized();

    const fromHex = options?.from ?? null;
    const valueHex =
      options?.value != null ? `0x${options.value.toString(16)}` : null;

    const result = await wasm.call(to, calldata, fromHex, valueHex);

    return {
      output: result.output as `0x${string}`,
      success: result.success,
      verified: result.verified,
    };
  }

  /**
   * Create an ABI-aware contract reader.
   *
   * @example
   * ```ts
   * const erc20 = client.contract('0x...', erc20Abi);
   * const balance = await erc20.read.balanceOf(['0xOwner']);
   * ```
   */
  contract<TAbi extends Abi>(
    address: `0x${string}`,
    abi: TAbi,
  ): ContractReader<TAbi> {
    return createContractReader(this, address, abi);
  }

  /**
   * Create a viem-compatible custom transport.
   *
   * @example
   * ```ts
   * import { createPublicClient } from 'viem';
   * import { polygon } from 'viem/chains';
   *
   * const client = createPublicClient({
   *   chain: polygon,
   *   transport: sqd.asViemTransport(),
   * });
   *
   * const result = await client.readContract({
   *   address: '0x...',
   *   abi: contractAbi,
   *   functionName: 'balanceOf',
   *   args: ['0xOwner'],
   * });
   * ```
   */
  asViemTransport() {
    return createEvmStateTransport(this);
  }
}
