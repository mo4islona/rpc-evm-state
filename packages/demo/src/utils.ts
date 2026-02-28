const Q96 = 2n ** 96n;

export function computePrice(
  sqrtPriceX96: bigint,
  decimals0: number,
  decimals1: number,
): { price0in1: number; price1in0: number } {
  const sqrtPrice = Number(sqrtPriceX96) / Number(Q96);
  const rawPrice = sqrtPrice * sqrtPrice;
  const decimalAdj = 10 ** (decimals0 - decimals1);
  const price0in1 = rawPrice * decimalAdj;
  return { price0in1, price1in0: 1 / price0in1 };
}

export function formatTick(
  tick: number,
  liquidityGross: bigint,
  liquidityNet: bigint,
): string {
  const sign = liquidityNet >= 0n ? "+" : "";
  return `  Tick ${String(tick).padStart(8)}: liquidityGross=${liquidityGross}, liquidityNet=${sign}${liquidityNet}`;
}
