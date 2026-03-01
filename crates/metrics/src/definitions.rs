use std::sync::LazyLock;

use prometheus::{
    Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts,
};

const NAMESPACE: &str = "evm_state";

// Bucket boundaries in seconds for RocksDB point lookups (10µs → 100ms).
const DB_LATENCY_BUCKETS: &[f64] = &[
    0.000_01, 0.000_05, 0.000_1, 0.000_25, 0.000_5, 0.001, 0.0025, 0.005, 0.01, 0.025, 0.05,
    0.1,
];

// Bucket boundaries in seconds for HTTP request latency (1ms → 5s).
const HTTP_LATENCY_BUCKETS: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0,
];

// ── DB read latency ─────────────────────────────────────────────

pub static DB_GET_ACCOUNT_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    Histogram::with_opts(
        HistogramOpts::new(
            "db_get_account_duration_seconds",
            "Latency of StateDb::get_account",
        )
        .namespace(NAMESPACE)
        .buckets(DB_LATENCY_BUCKETS.to_vec()),
    )
    .and_then(|h| {
        prometheus::register(Box::new(h.clone()))?;
        Ok(h)
    })
    .unwrap()
});

pub static DB_GET_STORAGE_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    Histogram::with_opts(
        HistogramOpts::new(
            "db_get_storage_duration_seconds",
            "Latency of StateDb::get_storage",
        )
        .namespace(NAMESPACE)
        .buckets(DB_LATENCY_BUCKETS.to_vec()),
    )
    .and_then(|h| {
        prometheus::register(Box::new(h.clone()))?;
        Ok(h)
    })
    .unwrap()
});

pub static DB_GET_CODE_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    Histogram::with_opts(
        HistogramOpts::new(
            "db_get_code_duration_seconds",
            "Latency of StateDb::get_code",
        )
        .namespace(NAMESPACE)
        .buckets(DB_LATENCY_BUCKETS.to_vec()),
    )
    .and_then(|h| {
        prometheus::register(Box::new(h.clone()))?;
        Ok(h)
    })
    .unwrap()
});

// ── HTTP request metrics ────────────────────────────────────────

pub static HTTP_REQUESTS_TOTAL: LazyLock<IntCounterVec> = LazyLock::new(|| {
    let opts = Opts::new("http_requests_total", "Total HTTP requests").namespace(NAMESPACE);
    let counter = IntCounterVec::new(opts, &["method", "path", "status"]).unwrap();
    prometheus::register(Box::new(counter.clone())).unwrap();
    counter
});

pub static HTTP_REQUEST_DURATION: LazyLock<HistogramVec> = LazyLock::new(|| {
    let opts = HistogramOpts::new(
        "http_request_duration_seconds",
        "HTTP request latency",
    )
    .namespace(NAMESPACE)
    .buckets(HTTP_LATENCY_BUCKETS.to_vec());
    let hist = HistogramVec::new(opts, &["method", "path"]).unwrap();
    prometheus::register(Box::new(hist.clone())).unwrap();
    hist
});

// ── State freshness ─────────────────────────────────────────────

pub static HEAD_BLOCK: LazyLock<IntGauge> = LazyLock::new(|| {
    let gauge = IntGauge::with_opts(
        Opts::new("head_block", "Latest block number in the state database").namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(gauge.clone())).unwrap();
    gauge
});

// ── WebSocket connections ───────────────────────────────────────

pub static WS_ACTIVE_CONNECTIONS: LazyLock<IntGauge> = LazyLock::new(|| {
    let gauge = IntGauge::with_opts(
        Opts::new(
            "ws_active_connections",
            "Currently active WebSocket connections",
        )
        .namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(gauge.clone())).unwrap();
    gauge
});

// ── Replayer metrics ────────────────────────────────────────────

pub static REPLAYER_BLOCKS_TOTAL: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::with_opts(
        Opts::new(
            "replayer_blocks_total",
            "Total blocks processed by the replayer",
        )
        .namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(counter.clone())).unwrap();
    counter
});

pub static REPLAYER_FETCH_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    let hist = Histogram::with_opts(
        HistogramOpts::new(
            "replayer_fetch_duration_seconds",
            "Per-block network fetch latency",
        )
        .namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(hist.clone())).unwrap();
    hist
});

pub static REPLAYER_EVM_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    let hist = Histogram::with_opts(
        HistogramOpts::new(
            "replayer_evm_duration_seconds",
            "Per-block EVM opcode execution latency (includes DB reads on cache miss)",
        )
        .namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(hist.clone())).unwrap();
    hist
});

pub static REPLAYER_COMMIT_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    let hist = Histogram::with_opts(
        HistogramOpts::new(
            "replayer_commit_duration_seconds",
            "Per-block CacheDB commit latency (merging tx state into block cache)",
        )
        .namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(hist.clone())).unwrap();
    hist
});

pub static REPLAYER_WRITE_DURATION: LazyLock<Histogram> = LazyLock::new(|| {
    let hist = Histogram::with_opts(
        HistogramOpts::new(
            "replayer_write_duration_seconds",
            "Per-block DB write latency",
        )
        .namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(hist.clone())).unwrap();
    hist
});

pub static REPLAYER_BYTES_DOWNLOADED: LazyLock<IntCounter> = LazyLock::new(|| {
    let counter = IntCounter::with_opts(
        Opts::new(
            "replayer_bytes_downloaded_total",
            "Total bytes downloaded from the portal",
        )
        .namespace(NAMESPACE),
    )
    .unwrap();
    prometheus::register(Box::new(counter.clone())).unwrap();
    counter
});
