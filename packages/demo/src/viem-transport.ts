/**
 * Uniswap V3 Pool Reader — viem drop-in transport demo
 *
 * This demo shows that existing viem code works unchanged when the
 * transport is swapped from an RPC provider to the EVM state service.
 */
import { createPublicClient } from "viem";
import { polygon } from "viem/chains";
import { EvmStateClient } from "@sqd/evm-state";
import {
  uniswapV3PoolAbi,
  POOL_ADDRESS,
  WMATIC_DECIMALS,
  USDC_DECIMALS,
} from "./abi.js";
import { computePrice, formatTick } from "./utils.js";

const endpoint = process.argv[2] ?? "http://localhost:3000";

// ── Create the viem-compatible transport ────────────────────────────
const sqd = new EvmStateClient({ endpoint });
const publicClient = createPublicClient({
  chain: polygon,
  transport: sqd.asViemTransport(),
});

console.log("=== Uniswap V3 Pool State (viem drop-in) ===");
console.log(`Pool:       ${POOL_ADDRESS}`);
console.log("Network:    Polygon PoS (chain ID 137)");
console.log(`Endpoint:   ${endpoint}`);
console.log();

let totalCalls = 0;

// ── slot0 ───────────────────────────────────────────────────────────
const slot0 = await publicClient.readContract({
  address: POOL_ADDRESS,
  abi: uniswapV3PoolAbi,
  functionName: "slot0",
});
totalCalls++;

const [sqrtPriceX96, tick, obsIdx, , , , unlocked] = slot0;
const price = computePrice(sqrtPriceX96, WMATIC_DECIMALS, USDC_DECIMALS);

console.log("--- slot0 ---");
console.log(`sqrtPriceX96:     ${sqrtPriceX96}`);
console.log(`Current tick:     ${tick}`);
console.log(`Observation idx:  ${obsIdx}`);
console.log(`Unlocked:         ${unlocked}`);
console.log();
console.log("--- Price ---");
console.log(`WMATIC/USDC:      ${price.price0in1.toFixed(4)} USDC`);
console.log(`USDC/WMATIC:      ${price.price1in0.toFixed(4)} WMATIC`);
console.log();

// ── liquidity ───────────────────────────────────────────────────────
const liquidity = await publicClient.readContract({
  address: POOL_ADDRESS,
  abi: uniswapV3PoolAbi,
  functionName: "liquidity",
});
totalCalls++;

console.log("--- Liquidity ---");
console.log(`Active liquidity: ${liquidity}`);
console.log();

// ── tickSpacing ─────────────────────────────────────────────────────
const tickSpacing = await publicClient.readContract({
  address: POOL_ADDRESS,
  abi: uniswapV3PoolAbi,
  functionName: "tickSpacing",
});
totalCalls++;

const spacing = Number(tickSpacing);
console.log("--- Tick Spacing ---");
console.log(`Spacing:          ${spacing}`);
console.log();

// ── Tick map scan ───────────────────────────────────────────────────
const currentTick = Number(tick);
const scanRange = 100;
const startTick = currentTick - scanRange * spacing;
const endTick = currentTick + scanRange * spacing;
const totalTicks = Math.floor((endTick - startTick) / spacing) + 1;

console.log(
  `--- Tick Map (${scanRange} ticks each direction, spacing=${spacing}) ---`,
);

const initializedTicks: {
  tick: number;
  liquidityGross: bigint;
  liquidityNet: bigint;
}[] = [];

for (let t = startTick; t <= endTick; t += spacing) {
  const tickData = await publicClient.readContract({
    address: POOL_ADDRESS,
    abi: uniswapV3PoolAbi,
    functionName: "ticks",
    args: [t],
  });
  totalCalls++;

  const [liqGross, liqNet, , , , , , initialized] = tickData;
  if (initialized) {
    initializedTicks.push({
      tick: t,
      liquidityGross: liqGross,
      liquidityNet: liqNet,
    });
  }
}

console.log(
  `Scanned ${totalTicks} tick indices, found ${initializedTicks.length} initialized:`,
);
for (const t of initializedTicks) {
  console.log(formatTick(t.tick, t.liquidityGross, t.liquidityNet));
}

console.log();
console.log(`Done. ${totalCalls} total calls to state service.`);
