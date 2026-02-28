import { custom } from "viem";
import type { EvmStateClient } from "./client.js";

/**
 * Create a viem-compatible custom transport backed by the EVM state service.
 *
 * Intercepts `eth_call` requests and routes them through
 * `EvmStateClient.call()`. Other RPC methods are not supported.
 *
 * @example
 * ```ts
 * import { createPublicClient } from 'viem';
 * import { polygon } from 'viem/chains';
 * import { EvmStateClient } from '@sqd/evm-state';
 *
 * const sqd = new EvmStateClient({ endpoint: 'http://localhost:3000' });
 * const publicClient = createPublicClient({
 *   chain: polygon,
 *   transport: sqd.asViemTransport(),
 * });
 *
 * // readContract now uses the state service instead of an RPC node
 * const balance = await publicClient.readContract({
 *   address: '0x...',
 *   abi: erc20Abi,
 *   functionName: 'balanceOf',
 *   args: ['0xOwner'],
 * });
 * ```
 */
export function createEvmStateTransport(client: EvmStateClient) {
  return custom({
    async request({ method, params }: { method: string; params?: unknown[] }) {
      if (method === "eth_call") {
        const callObj = (params as Record<string, string>[])[0];
        const to = callObj.to as `0x${string}`;
        const data = (callObj.data ?? "0x") as `0x${string}`;

        const result = await client.call(to, data, {
          from: callObj.from as `0x${string}` | undefined,
          value:
            callObj.value != null ? BigInt(callObj.value) : undefined,
        });

        if (!result.success) {
          throw new Error(`eth_call reverted: ${result.output}`);
        }

        return result.output;
      }

      throw new Error(
        `EVM state transport does not support method: ${method}`,
      );
    },
  });
}
