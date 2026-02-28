use std::sync::Arc;

use alloy_primitives::{B256, U256};
use criterion::{Criterion, criterion_group, criterion_main};
use evm_state_bench::{account_address, setup_populated_db, setup_tick_scan_db, start_server};
use evm_state_client::{RemoteDB, StateClient, TrustMode};
use revm::database_interface::DatabaseRef;
use revm::primitives::hardfork::SpecId;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

// ── Single call latency ─────────────────────────────────────────────

fn single_call_latency(c: &mut Criterion) {
    let rt = runtime();
    let (_dir, state) = setup_populated_db(1, 10);
    let base_url = rt.block_on(start_server(Arc::clone(&state)));
    let contract = account_address(0);

    let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::TrustServer);

    c.bench_function("single_call_latency/trust_server", |b| {
        b.iter(|| {
            let result = client.call(contract, &[], None, None).unwrap();
            assert!(result.success);
        });
    });

    let client_verify = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::Verify);

    c.bench_function("single_call_latency/verify", |b| {
        b.iter(|| {
            let result = client_verify.call(contract, &[], None, None).unwrap();
            assert!(result.success);
            assert_eq!(result.verified, Some(true));
        });
    });
}

// ── 100 parallel reads throughput ───────────────────────────────────

fn parallel_reads_throughput(c: &mut Criterion) {
    let rt = runtime();
    let (_dir, state) = setup_populated_db(100, 1);
    let base_url = rt.block_on(start_server(Arc::clone(&state)));

    c.bench_function("parallel_reads/100_storage_reads", |b| {
        b.iter(|| {
            let remote = RemoteDB::new(&base_url);
            for i in 0..100 {
                let addr = account_address(i);
                let value = remote.storage_ref(addr, U256::ZERO).unwrap();
                assert_ne!(value, U256::ZERO);
            }
        });
    });

    // Also benchmark with prefetch (batch style) — read accounts
    c.bench_function("parallel_reads/100_account_reads", |b| {
        b.iter(|| {
            let remote = RemoteDB::new(&base_url);
            for i in 0..100 {
                let addr = account_address(i);
                let info = remote.basic_ref(addr).unwrap();
                assert!(info.is_some());
            }
        });
    });
}

// ── 200-tick Uniswap scan ───────────────────────────────────────────

fn uniswap_tick_scan(c: &mut Criterion) {
    let rt = runtime();
    let (_dir, state, contract) = setup_tick_scan_db(200);
    let base_url = rt.block_on(start_server(Arc::clone(&state)));

    // Single prefetch call that executes a contract reading 200 slots
    let client = StateClient::new(&base_url, SpecId::SHANGHAI, TrustMode::TrustServer);

    c.bench_function("uniswap_tick_scan/prefetch_200_slots", |b| {
        b.iter(|| {
            let result = client.call(contract, &[], None, None).unwrap();
            assert!(result.success);
        });
    });

    // Compare: 200 individual storage reads
    c.bench_function("uniswap_tick_scan/200_individual_reads", |b| {
        b.iter(|| {
            let remote = RemoteDB::new(&base_url);
            for slot_idx in 0..200 {
                let slot = B256::from(U256::from(slot_idx));
                let value = remote
                    .storage_ref(contract, U256::from_be_bytes(slot.0))
                    .unwrap();
                assert_ne!(value, U256::ZERO);
            }
        });
    });
}

criterion_group!(
    benches,
    single_call_latency,
    parallel_reads_throughput,
    uniswap_tick_scan
);
criterion_main!(benches);
