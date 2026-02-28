import { parseAbi } from "viem";

/** Uniswap V3 WMATIC/USDC 0.05% pool on Polygon */
export const POOL_ADDRESS =
  "0xa374094527e1673a86de625aa7d246023b9e3d71" as const;

export const WMATIC_DECIMALS = 18;
export const USDC_DECIMALS = 6;

export const uniswapV3PoolAbi = parseAbi([
  "function slot0() view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked)",
  "function liquidity() view returns (uint128)",
  "function tickSpacing() view returns (int24)",
  "function ticks(int24) view returns (uint128 liquidityGross, int128 liquidityNet, uint256 feeGrowthOutside0X128, uint256 feeGrowthOutside1X128, int56 tickCumulativeOutside, uint160 secondsPerLiquidityOutsideX128, uint32 secondsOutside, bool initialized)",
]);
