use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy_primitives::{B256, U256};
use evm_state_bench::{
    RpcClient, account_address, fmt_duration, percentile, setup_populated_db, setup_tick_scan_db,
    start_server,
};
use evm_state_client::{RemoteDB, StateClient, TrustMode};
use revm::database_interface::DatabaseRef;
use revm::primitives::hardfork::SpecId;

struct BenchResult {
    name: String,
    service_latencies: Vec<Duration>,
    rpc_latencies: Option<Vec<Duration>>,
}

impl BenchResult {
    fn service_p50(&self) -> Duration {
        percentile(&self.service_latencies, 50.0)
    }
    fn service_p95(&self) -> Duration {
        percentile(&self.service_latencies, 95.0)
    }
    fn service_p99(&self) -> Duration {
        percentile(&self.service_latencies, 99.0)
    }
    fn rpc_p50(&self) -> Option<Duration> {
        self.rpc_latencies.as_ref().map(|l| percentile(l, 50.0))
    }
    fn rpc_p99(&self) -> Option<Duration> {
        self.rpc_latencies.as_ref().map(|l| percentile(l, 99.0))
    }
    fn speedup(&self) -> Option<f64> {
        let rpc_p50 = self.rpc_p50()?;
        let svc_p50 = self.service_p50();
        if svc_p50.is_zero() {
            return None;
        }
        Some(rpc_p50.as_nanos() as f64 / svc_p50.as_nanos() as f64)
    }
}

fn main() {
    let iterations: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(100);

    let rpc = RpcClient::from_env();

    eprintln!("Running benchmarks with {} iterations...", iterations);
    if rpc.is_some() {
        eprintln!("RPC_URL is set — will compare against external RPC.");
    } else {
        eprintln!("RPC_URL not set — skipping RPC comparison.");
    }
    eprintln!();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let results = vec![
        bench_single_call(&rt, iterations, rpc.as_ref()),
        bench_100_reads(&rt, iterations, rpc.as_ref()),
        bench_tick_scan(&rt, iterations, rpc.as_ref()),
    ];

    print_markdown_report(&results);
}

fn bench_single_call(
    rt: &tokio::runtime::Runtime,
    iterations: usize,
    rpc: Option<&RpcClient>,
) -> BenchResult {
    eprint!("  single call latency...");
    let (_dir, state) = setup_populated_db(1, 10);
    let base_url = rt.block_on(start_server(Arc::clone(&state)));
    let contract = account_address(0);

    let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::TrustServer);

    // Warm up
    let _ = client.call(contract, &[], None, None);

    let mut latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let result = client.call(contract, &[], None, None).unwrap();
        latencies.push(start.elapsed());
        assert!(result.success);
    }
    latencies.sort();

    let rpc_latencies = rpc.map(|rpc| {
        let mut lats = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            let _ = rpc.eth_call(&contract, &[]);
            lats.push(start.elapsed());
        }
        lats.sort();
        lats
    });

    eprintln!(" done");

    BenchResult {
        name: "Single call".to_string(),
        service_latencies: latencies,
        rpc_latencies,
    }
}

fn bench_100_reads(
    rt: &tokio::runtime::Runtime,
    iterations: usize,
    rpc: Option<&RpcClient>,
) -> BenchResult {
    eprint!("  100 parallel reads...");
    let (_dir, state) = setup_populated_db(100, 1);
    let base_url = rt.block_on(start_server(Arc::clone(&state)));

    // Warm up
    {
        let remote = RemoteDB::new(&base_url);
        let _ = remote.storage_ref(account_address(0), U256::ZERO);
    }

    let mut latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let remote = RemoteDB::new(&base_url);
        let start = Instant::now();
        for i in 0..100 {
            let addr = account_address(i);
            let _ = remote.storage_ref(addr, U256::ZERO).unwrap();
        }
        latencies.push(start.elapsed());
    }
    latencies.sort();

    let rpc_latencies = rpc.map(|rpc| {
        let mut lats = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            for i in 0..100 {
                let addr = account_address(i);
                let _ = rpc.get_storage_at(&addr, &B256::ZERO);
            }
            lats.push(start.elapsed());
        }
        lats.sort();
        lats
    });

    eprintln!(" done");

    BenchResult {
        name: "100 reads".to_string(),
        service_latencies: latencies,
        rpc_latencies,
    }
}

fn bench_tick_scan(
    rt: &tokio::runtime::Runtime,
    iterations: usize,
    rpc: Option<&RpcClient>,
) -> BenchResult {
    eprint!("  200-tick scan...");
    let (_dir, state, contract) = setup_tick_scan_db(200);
    let base_url = rt.block_on(start_server(Arc::clone(&state)));

    let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::TrustServer);

    // Warm up
    let _ = client.call(contract, &[], None, None);

    let mut latencies = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let result = client.call(contract, &[], None, None).unwrap();
        latencies.push(start.elapsed());
        assert!(result.success);
    }
    latencies.sort();

    let rpc_latencies = rpc.map(|rpc| {
        let mut lats = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let start = Instant::now();
            for slot_idx in 0..200 {
                let slot = B256::from(U256::from(slot_idx));
                let _ = rpc.get_storage_at(&contract, &slot);
            }
            lats.push(start.elapsed());
        }
        lats.sort();
        lats
    });

    eprintln!(" done");

    BenchResult {
        name: "200-tick scan".to_string(),
        service_latencies: latencies,
        rpc_latencies,
    }
}

fn print_markdown_report(results: &[BenchResult]) {
    println!("# EVM State Service — Benchmark Report\n");
    println!(
        "| Scenario | Service p50 | Service p95 | Service p99 | RPC p50 | RPC p99 | Speedup |"
    );
    println!(
        "|----------|-------------|-------------|-------------|---------|---------|---------|"
    );

    for r in results {
        let rpc_p50 = r
            .rpc_p50()
            .map(|d| fmt_duration(d))
            .unwrap_or_else(|| "—".to_string());
        let rpc_p99 = r
            .rpc_p99()
            .map(|d| fmt_duration(d))
            .unwrap_or_else(|| "—".to_string());
        let speedup = r
            .speedup()
            .map(|s| format!("{:.1}x", s))
            .unwrap_or_else(|| "—".to_string());

        println!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            r.name,
            fmt_duration(r.service_p50()),
            fmt_duration(r.service_p95()),
            fmt_duration(r.service_p99()),
            rpc_p50,
            rpc_p99,
            speedup,
        );
    }

    println!();
    println!("*Generated by `cargo run -p evm-state-bench --bin report`*");
}
