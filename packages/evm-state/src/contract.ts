import {
  type Abi,
  type AbiFunction,
  encodeFunctionData,
  decodeFunctionResult,
} from "viem";
import type { EvmStateClient, CallOptions } from "./client.js";

/**
 * ABI-aware contract reader with typed `read` methods.
 *
 * Methods are generated dynamically via Proxy from the ABI's
 * view/pure functions.
 */
export interface ContractReader<TAbi extends Abi> {
  read: ContractReadMethods<TAbi>;
}

/**
 * Maps ABI view/pure function names to async methods.
 *
 * Each method accepts an array of arguments matching the ABI function
 * inputs and returns the decoded result.
 */
export type ContractReadMethods<TAbi extends Abi> = {
  [K in ExtractViewFunctionNames<TAbi>]: (
    args?: readonly unknown[],
    options?: CallOptions,
  ) => Promise<unknown>;
};

/** Extract names of view/pure functions from an ABI. */
type ExtractViewFunctionNames<TAbi extends Abi> = Extract<
  TAbi[number],
  {
    type: "function";
    stateMutability: "view" | "pure";
  }
> extends { name: infer N extends string }
  ? N
  : never;

/**
 * Create a contract reader that encodes calls via viem and routes
 * them through the EvmStateClient.
 */
export function createContractReader<TAbi extends Abi>(
  client: EvmStateClient,
  address: `0x${string}`,
  abi: TAbi,
): ContractReader<TAbi> {
  const viewFunctions = (abi as readonly unknown[]).filter(
    (item): item is AbiFunction =>
      typeof item === "object" &&
      item !== null &&
      "type" in item &&
      item.type === "function" &&
      "stateMutability" in item &&
      (item.stateMutability === "view" || item.stateMutability === "pure"),
  );

  const read = new Proxy(
    {},
    {
      get(_target, prop: string) {
        const fn = viewFunctions.find((f) => f.name === prop);
        if (!fn) {
          return undefined;
        }

        return async (
          args?: readonly unknown[],
          options?: CallOptions,
        ): Promise<unknown> => {
          const calldata = encodeFunctionData({
            abi: abi as Abi,
            functionName: prop,
            args: args ?? [],
          });

          const result = await client.call(
            address,
            calldata as `0x${string}`,
            options,
          );

          if (!result.success) {
            throw new Error(
              `Contract call reverted: ${prop}() at ${address}`,
            );
          }

          return decodeFunctionResult({
            abi: abi as Abi,
            functionName: prop,
            data: result.output,
          });
        };
      },

      has(_target, prop: string) {
        return viewFunctions.some((f) => f.name === prop);
      },
    },
  ) as ContractReadMethods<TAbi>;

  return { read };
}
