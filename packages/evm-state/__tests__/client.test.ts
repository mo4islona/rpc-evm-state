import { describe, it, expect, vi } from "vitest";
import {
  encodeFunctionData,
  decodeFunctionResult,
  parseAbi,
  createPublicClient,
} from "viem";
import { EvmStateClient, type WasmClient } from "../src/index.js";

// ── Minimal ERC20 ABI for testing ─────────────────────────────────

const erc20Abi = parseAbi([
  "function balanceOf(address owner) view returns (uint256)",
  "function decimals() view returns (uint8)",
  "function totalSupply() view returns (uint256)",
  "function transfer(address to, uint256 amount) returns (bool)",
]);

// ── Mock WASM client ──────────────────────────────────────────────

function createMockWasm(
  responses: Map<string, { output: string; success: boolean }>,
): WasmClient {
  return {
    async call(to, calldata, _from, _value) {
      const key = `${to}:${calldata}`;
      const resp = responses.get(key);
      if (resp) {
        return { ...resp, verified: null };
      }
      // Default: return zeros
      return {
        output:
          "0x0000000000000000000000000000000000000000000000000000000000000000",
        success: true,
        verified: null,
      };
    },
    async prefetch(to, calldata) {
      const key = `${to}:${calldata}`;
      const resp = responses.get(key);
      return {
        result:
          resp?.output ??
          "0x0000000000000000000000000000000000000000000000000000000000000000",
        success: resp?.success ?? true,
      };
    },
  };
}

// ── Tests ─────────────────────────────────────────────────────────

describe("EvmStateClient", () => {
  it("call() returns hex output", async () => {
    const mock = createMockWasm(new Map());
    const client = EvmStateClient.fromWasmClient(mock);

    const result = await client.call("0x1234567890abcdef1234567890abcdef12345678", "0x");
    expect(result.output).toMatch(/^0x/);
    expect(result.success).toBe(true);
    expect(result.verified).toBeNull();
  });

  it("call() forwards from and value options", async () => {
    const callSpy = vi.fn().mockResolvedValue({
      output: "0x01",
      success: true,
      verified: null,
    });
    const mock: WasmClient = {
      call: callSpy,
      prefetch: vi.fn(),
    };
    const client = EvmStateClient.fromWasmClient(mock);

    await client.call(
      "0x1234567890abcdef1234567890abcdef12345678",
      "0xdeadbeef",
      {
        from: "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        value: 1000n,
      },
    );

    expect(callSpy).toHaveBeenCalledWith(
      "0x1234567890abcdef1234567890abcdef12345678",
      "0xdeadbeef",
      "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
      "0x3e8",
    );
  });
});

describe("ABI encoding/decoding", () => {
  it("encodeFunctionData produces correct calldata for balanceOf", () => {
    const calldata = encodeFunctionData({
      abi: erc20Abi,
      functionName: "balanceOf",
      args: ["0x0000000000000000000000000000000000C0FFEE"],
    });

    // balanceOf(address) selector = 0x70a08231
    expect(calldata).toMatch(/^0x70a08231/);
  });

  it("decodeFunctionResult decodes uint256 balance", () => {
    const encoded =
      "0x00000000000000000000000000000000000000000000000000000000000003e8";
    const decoded = decodeFunctionResult({
      abi: erc20Abi,
      functionName: "balanceOf",
      data: encoded,
    });

    expect(decoded).toBe(1000n);
  });

  it("decodeFunctionResult decodes uint8 decimals", () => {
    const encoded =
      "0x0000000000000000000000000000000000000000000000000000000000000012";
    const decoded = decodeFunctionResult({
      abi: erc20Abi,
      functionName: "decimals",
      data: encoded,
    });

    expect(decoded).toBe(18);
  });
});

describe("contract().read", () => {
  it("encodes and decodes balanceOf correctly", async () => {
    const owner = "0x0000000000000000000000000000000000C0FFEE";

    // Pre-compute the expected calldata
    const expectedCalldata = encodeFunctionData({
      abi: erc20Abi,
      functionName: "balanceOf",
      args: [owner],
    });

    // Mock response: 1000 as uint256
    const responses = new Map<string, { output: string; success: boolean }>();
    const contractAddr = "0x1234567890abcdef1234567890abcdef12345678";
    responses.set(`${contractAddr}:${expectedCalldata}`, {
      output:
        "0x00000000000000000000000000000000000000000000000000000000000003e8",
      success: true,
    });

    const mock = createMockWasm(responses);
    const client = EvmStateClient.fromWasmClient(mock);

    const erc20 = client.contract(
      contractAddr as `0x${string}`,
      erc20Abi,
    );
    const balance = await erc20.read.balanceOf([owner]);

    expect(balance).toBe(1000n);
  });

  it("encodes and decodes decimals correctly", async () => {
    const expectedCalldata = encodeFunctionData({
      abi: erc20Abi,
      functionName: "decimals",
    });

    const contractAddr = "0x1234567890abcdef1234567890abcdef12345678";
    const responses = new Map<string, { output: string; success: boolean }>();
    responses.set(`${contractAddr}:${expectedCalldata}`, {
      output:
        "0x0000000000000000000000000000000000000000000000000000000000000012",
      success: true,
    });

    const mock = createMockWasm(responses);
    const client = EvmStateClient.fromWasmClient(mock);

    const erc20 = client.contract(
      contractAddr as `0x${string}`,
      erc20Abi,
    );
    const decimals = await erc20.read.decimals();

    expect(decimals).toBe(18);
  });

  it("throws on reverted call", async () => {
    const contractAddr = "0x1234567890abcdef1234567890abcdef12345678";

    // Mock that returns failure for any call
    const mock: WasmClient = {
      async call() {
        return { output: "0x", success: false, verified: null };
      },
      async prefetch() {
        return { result: "0x", success: false };
      },
    };

    const client = EvmStateClient.fromWasmClient(mock);
    const erc20 = client.contract(
      contractAddr as `0x${string}`,
      erc20Abi,
    );

    await expect(erc20.read.decimals()).rejects.toThrow("reverted");
  });
});

describe("asViemTransport()", () => {
  it("intercepts eth_call and returns result", async () => {
    const contractAddr =
      "0x1234567890abcdef1234567890abcdef12345678" as `0x${string}`;
    const expectedCalldata = encodeFunctionData({
      abi: erc20Abi,
      functionName: "totalSupply",
    });

    const responses = new Map<string, { output: string; success: boolean }>();
    responses.set(`${contractAddr}:${expectedCalldata}`, {
      output:
        "0x0000000000000000000000000000000000000000000000000000000005f5e100",
      success: true,
    });

    const mock = createMockWasm(responses);
    const client = EvmStateClient.fromWasmClient(mock);

    const publicClient = createPublicClient({
      transport: client.asViemTransport(),
    });

    const result = await publicClient.readContract({
      address: contractAddr,
      abi: erc20Abi,
      functionName: "totalSupply",
    });

    expect(result).toBe(100_000_000n);
  });

  it("throws for unsupported methods", async () => {
    const mock = createMockWasm(new Map());
    const client = EvmStateClient.fromWasmClient(mock);

    const publicClient = createPublicClient({
      transport: client.asViemTransport(),
    });

    await expect(publicClient.getBlockNumber()).rejects.toThrow(
      "does not support",
    );
  });
});
