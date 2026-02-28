use alloy_primitives::{Address, U256};
use alloy_sol_types::{sol, SolCall};
use anyhow::{ensure, Context, Result};
use evm_state_client::{StateClient, TrustMode};
use revm::primitives::hardfork::SpecId;

sol! {
    interface IUniswapV3Pool {
        function slot0() external view returns (
            uint160 sqrtPriceX96,
            int24 tick,
            uint16 observationIndex,
            uint16 observationCardinality,
            uint16 observationCardinalityNext,
            uint8 feeProtocol,
            bool unlocked
        );

        function liquidity() external view returns (uint128);

        function tickSpacing() external view returns (int24);

        function ticks(int24 tick) external view returns (
            uint128 liquidityGross,
            int128 liquidityNet,
            uint256 feeGrowthOutside0X128,
            uint256 feeGrowthOutside1X128,
            int56 tickCumulativeOutside,
            uint160 secondsPerLiquidityOutsideX128,
            uint32 secondsOutside,
            bool initialized
        );
    }
}

/// Default: Uniswap V3 WMATIC/USDC 0.05% pool on Polygon
const DEFAULT_POOL: &str = "0xa374094527e1673a86de625aa7d246023b9e3d71";
const WMATIC_DECIMALS: u8 = 18;
const USDC_DECIMALS: u8 = 6;

fn compute_price(sqrt_price_x96: U256, decimals0: u8, decimals1: u8) -> (f64, f64) {
    let sqrt_price = sqrt_price_x96.to::<u128>() as f64 / 2.0_f64.powi(96);
    let raw_price = sqrt_price * sqrt_price;
    let decimal_adj = 10.0_f64.powi(decimals0 as i32 - decimals1 as i32);
    let price0in1 = raw_price * decimal_adj;
    (price0in1, 1.0 / price0in1)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <endpoint> [pool_address]", args[0]);
        eprintln!("  endpoint     - URL of the EVM state API (e.g. http://localhost:3000)");
        eprintln!("  pool_address - Uniswap V3 pool address (default: WMATIC/USDC 0.05%)");
        std::process::exit(1);
    }

    let endpoint = &args[1];
    let pool_address: Address = args
        .get(2)
        .map(|s| s.as_str())
        .unwrap_or(DEFAULT_POOL)
        .parse()
        .context("invalid pool address")?;

    let client = StateClient::new(endpoint, SpecId::CANCUN, TrustMode::TrustServer);
    let mut total_calls = 0u32;

    println!("=== Uniswap V3 Pool State ===");
    println!("Pool:       {pool_address}");
    println!("Network:    Polygon PoS (chain ID 137)");
    println!("Endpoint:   {endpoint}");
    println!();

    // --- slot0 ---
    let calldata = IUniswapV3Pool::slot0Call {}.abi_encode();
    let result = client.call(pool_address, &calldata, None, None)?;
    total_calls += 1;
    ensure!(result.success, "slot0() call reverted");

    let slot0 = IUniswapV3Pool::slot0Call::abi_decode_returns(&result.output)
        .context("failed to decode slot0 return")?;

    let current_tick: i32 = slot0.tick.as_i32();
    let (price0in1, price1in0) =
        compute_price(U256::from(slot0.sqrtPriceX96), WMATIC_DECIMALS, USDC_DECIMALS);

    println!("--- slot0 ---");
    println!("sqrtPriceX96:     {}", slot0.sqrtPriceX96);
    println!("Current tick:     {current_tick}");
    println!("Observation idx:  {}", slot0.observationIndex);
    println!("Unlocked:         {}", slot0.unlocked);
    println!();
    println!("--- Price ---");
    println!("WMATIC/USDC:      {price0in1:.4} USDC");
    println!("USDC/WMATIC:      {price1in0:.4} WMATIC");
    println!();

    // --- liquidity ---
    let calldata = IUniswapV3Pool::liquidityCall {}.abi_encode();
    let result = client.call(pool_address, &calldata, None, None)?;
    total_calls += 1;
    ensure!(result.success, "liquidity() call reverted");

    let liquidity: u128 =
        IUniswapV3Pool::liquidityCall::abi_decode_returns(&result.output)
            .context("failed to decode liquidity return")?;

    println!("--- Liquidity ---");
    println!("Active liquidity: {liquidity}");
    println!();

    // --- tickSpacing ---
    let calldata = IUniswapV3Pool::tickSpacingCall {}.abi_encode();
    let result = client.call(pool_address, &calldata, None, None)?;
    total_calls += 1;
    ensure!(result.success, "tickSpacing() call reverted");

    let tick_spacing =
        IUniswapV3Pool::tickSpacingCall::abi_decode_returns(&result.output)
            .context("failed to decode tickSpacing return")?;

    let spacing: i32 = tick_spacing.as_i32();
    println!("--- Tick Spacing ---");
    println!("Spacing:          {spacing}");
    println!();

    // --- Tick scan ---
    let scan_range = 100i32;
    let start_tick = current_tick - scan_range * spacing;
    let end_tick = current_tick + scan_range * spacing;
    let total_ticks = ((end_tick - start_tick) / spacing + 1) as u32;

    println!("--- Tick Map ({scan_range} ticks each direction, spacing={spacing}) ---");

    let mut initialized_ticks = Vec::new();
    let mut tick = start_tick;
    while tick <= end_tick {
        let calldata = IUniswapV3Pool::ticksCall {
            tick: alloy_primitives::aliases::I24::try_from(tick)
                .context("tick out of int24 range")?,
        }
        .abi_encode();

        let result = client.call(pool_address, &calldata, None, None)?;
        total_calls += 1;

        if result.success {
            let tick_data =
                IUniswapV3Pool::ticksCall::abi_decode_returns(&result.output)
                    .context("failed to decode ticks return")?;

            if tick_data.initialized {
                initialized_ticks.push((tick, tick_data));
            }
        }

        tick += spacing;
    }

    println!(
        "Scanned {total_ticks} tick indices, found {} initialized:",
        initialized_ticks.len()
    );
    for (tick_idx, data) in &initialized_ticks {
        let net = data.liquidityNet;
        let sign = if net >= 0 { "+" } else { "" };
        println!(
            "  Tick {tick_idx:>8}: liquidityGross={}, liquidityNet={sign}{net}",
            data.liquidityGross,
        );
    }

    println!();
    println!("Done. {total_calls} total calls to state service.");

    Ok(())
}
