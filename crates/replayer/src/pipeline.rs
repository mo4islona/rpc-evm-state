use crate::{apply_state_diffs, replay_block, Error, Result};
use evm_state_chain_spec::ChainSpec;
use evm_state_data_types::Block;
use evm_state_db::StateDb;
use futures::stream::Stream;
use futures::StreamExt;
use std::fmt;
use std::time::Instant;

/// Controls how blocks are processed in the pipeline.
pub enum PipelineMode {
    /// Execute transactions with revm (normal replay).
    Replay(ChainSpec),
    /// Apply state diffs directly from the portal (no EVM execution).
    StateDiffs,
}

/// Summary statistics from a pipeline run.
#[derive(Debug, Clone)]
pub struct PipelineStats {
    /// Number of blocks successfully processed in this run.
    pub blocks_processed: u64,
    /// First block number processed (`None` if no blocks were processed).
    pub first_block: Option<u64>,
    /// Last block number processed (`None` if no blocks were processed).
    pub last_block: Option<u64>,
    /// Total wall-clock time for the run.
    pub elapsed: std::time::Duration,
}

impl PipelineStats {
    /// Blocks per second throughput.
    pub fn blocks_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            self.blocks_processed as f64 / secs
        } else {
            0.0
        }
    }
}

/// Progress information passed to the callback after each block.
#[derive(Debug, Clone)]
pub struct BlockProgress {
    /// The block number that was just processed.
    pub block_number: u64,
    /// Number of transactions in this block.
    pub tx_count: usize,
    /// Number of blocks processed so far in this pipeline run.
    pub blocks_processed: u64,
    /// Wall-clock time since the pipeline started.
    pub elapsed: std::time::Duration,
}

impl BlockProgress {
    /// Blocks per second throughput.
    pub fn blocks_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64();
        if secs > 0.0 {
            self.blocks_processed as f64 / secs
        } else {
            0.0
        }
    }
}

/// Run the replay pipeline: consume blocks from a stream and replay each
/// one against the state database.
///
/// Blocks are processed sequentially. Each block's state changes are
/// committed atomically by [`replay_block`]. The pipeline resumes from the
/// database's `head_block` — any blocks in the stream at or below
/// `head_block` are silently skipped.
///
/// The `on_progress` callback is called after each block. Return `false`
/// to request a graceful shutdown (the pipeline stops after the current
/// block). Since each block commit is atomic, dropping the future between
/// blocks is always safe.
pub async fn run_pipeline<S, E, F>(
    db: &StateDb,
    mode: &PipelineMode,
    blocks: S,
    mut on_progress: F,
) -> Result<PipelineStats>
where
    S: Stream<Item = std::result::Result<Block, E>>,
    E: fmt::Display,
    F: FnMut(&BlockProgress) -> bool,
{
    futures::pin_mut!(blocks);

    let head_block = db.get_head_block()?;
    let start = Instant::now();
    let mut stats = PipelineStats {
        blocks_processed: 0,
        first_block: None,
        last_block: None,
        elapsed: std::time::Duration::ZERO,
    };

    while let Some(block_result) = blocks.next().await {
        let block = block_result.map_err(|e| Error::Source(e.to_string()))?;

        // Skip blocks already processed (resume safety).
        if let Some(head) = head_block {
            if block.header.number <= head {
                continue;
            }
        }

        // Continuity check: after the first processed block, each subsequent
        // block must be exactly last + 1 to prevent silent data corruption.
        if let Some(last) = stats.last_block {
            if block.header.number != last + 1 {
                return Err(Error::Source(format!(
                    "block gap: expected {} but got {}",
                    last + 1,
                    block.header.number
                )));
            }
        }

        let tx_count = match mode {
            PipelineMode::Replay(chain_spec) => {
                let count = block.transactions.len();
                replay_block(db, &block, chain_spec)?;
                count
            }
            PipelineMode::StateDiffs => {
                let count = block.state_diffs.len();
                apply_state_diffs(db, &block)?;
                count
            }
        };

        stats.blocks_processed += 1;
        if stats.first_block.is_none() {
            stats.first_block = Some(block.header.number);
        }
        stats.last_block = Some(block.header.number);
        stats.elapsed = start.elapsed();

        let progress = BlockProgress {
            block_number: block.header.number,
            tx_count,
            blocks_processed: stats.blocks_processed,
            elapsed: stats.elapsed,
        };

        if !on_progress(&progress) {
            break;
        }
    }

    stats.elapsed = start.elapsed();
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use alloy_primitives::{address, keccak256, Bytes, B256, U256};
    use evm_state_common::AccountInfo;

    #[tokio::test]
    async fn pipeline_replays_range_of_blocks() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();
        let blocks: Vec<std::result::Result<_, std::convert::Infallible>> =
            (1..=5).map(|n| Ok(simple_block(n, vec![]))).collect();

        let stats = run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |_| true)
            .await
            .unwrap();

        assert_eq!(stats.blocks_processed, 5);
        assert_eq!(stats.first_block, Some(1));
        assert_eq!(stats.last_block, Some(5));
        assert_eq!(db.get_head_block().unwrap().unwrap(), 5);
    }

    #[tokio::test]
    async fn pipeline_resumes_from_head_block() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();

        // Pre-set head_block to 3
        db.set_head_block(3).unwrap();

        let blocks: Vec<std::result::Result<_, std::convert::Infallible>> =
            (1..=6).map(|n| Ok(simple_block(n, vec![]))).collect();

        let stats = run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |_| true)
            .await
            .unwrap();

        // Should skip blocks 1-3, process 4-6
        assert_eq!(stats.blocks_processed, 3);
        assert_eq!(stats.first_block, Some(4));
        assert_eq!(stats.last_block, Some(6));
        assert_eq!(db.get_head_block().unwrap().unwrap(), 6);
    }

    #[tokio::test]
    async fn pipeline_stops_on_callback_false() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();
        let blocks: Vec<std::result::Result<_, std::convert::Infallible>> =
            (1..=100).map(|n| Ok(simple_block(n, vec![]))).collect();

        let stats = run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |p| {
            p.blocks_processed < 5
        })
        .await
        .unwrap();

        assert_eq!(stats.blocks_processed, 5);
        assert_eq!(stats.last_block, Some(5));
        assert_eq!(db.get_head_block().unwrap().unwrap(), 5);
    }

    #[tokio::test]
    async fn pipeline_propagates_source_error() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();

        let blocks: Vec<std::result::Result<_, &str>> = vec![
            Ok(simple_block(1, vec![])),
            Ok(simple_block(2, vec![])),
            Err("connection lost"),
            Ok(simple_block(4, vec![])),
        ];

        let result = run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |_| true).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::Source(_)));
        // Blocks 1-2 were committed before the error
        assert_eq!(db.get_head_block().unwrap().unwrap(), 2);
    }

    #[tokio::test]
    async fn pipeline_empty_stream() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();
        let blocks: Vec<std::result::Result<_, std::convert::Infallible>> = vec![];

        let stats = run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |_| true)
            .await
            .unwrap();

        assert_eq!(stats.blocks_processed, 0);
        assert!(stats.first_block.is_none());
        assert!(stats.last_block.is_none());
        assert!(db.get_head_block().unwrap().is_none());
    }

    #[tokio::test]
    async fn pipeline_detects_block_gap() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();
        let blocks: Vec<std::result::Result<_, std::convert::Infallible>> = vec![
            Ok(simple_block(1, vec![])),
            Ok(simple_block(2, vec![])),
            Ok(simple_block(5, vec![])), // gap: 3 and 4 missing
        ];

        let result = run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |_| true).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Source(_)));
        assert!(err.to_string().contains("block gap"));
        // Blocks 1-2 committed before the gap was detected
        assert_eq!(db.get_head_block().unwrap().unwrap(), 2);
    }

    #[tokio::test]
    async fn pipeline_accumulates_state_across_blocks() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();
        let caller = address!("0000000000000000000000000000000000000001");

        // Contract: reads slot 0, adds 1, writes back to slot 0.
        let inc_bytecode = vec![
            0x60, 0x00, // PUSH1 0x00
            0x54,       // SLOAD
            0x60, 0x01, // PUSH1 0x01
            0x01,       // ADD
            0x60, 0x00, // PUSH1 0x00
            0x55,       // SSTORE
            0x00,       // STOP
        ];
        let inc_hash = keccak256(&inc_bytecode);
        let inc_addr = address!("0000000000000000000000000000000000001111");

        db.set_account(
            &inc_addr,
            &AccountInfo {
                nonce: 0,
                balance: U256::ZERO,
                code_hash: inc_hash.into(),
            },
        )
        .unwrap();
        db.set_code(&inc_hash.into(), &inc_bytecode).unwrap();
        db.set_account(
            &caller,
            &AccountInfo {
                nonce: 0,
                balance: U256::from(1_000_000_000u64),
                code_hash: B256::ZERO,
            },
        )
        .unwrap();

        // Block 1: call increment (slot 0 goes 0 -> 1), nonce 0
        let tx0 = simple_tx(0, caller, Some(inc_addr), Bytes::new());
        // Block 2: call increment (slot 0 goes 1 -> 2), nonce 1
        let mut tx1 = simple_tx(0, caller, Some(inc_addr), Bytes::new());
        tx1.nonce = Some(1);

        let blocks: Vec<std::result::Result<_, std::convert::Infallible>> = vec![
            Ok(simple_block(1, vec![tx0])),
            Ok(simple_block(2, vec![tx1])),
        ];

        let stats = run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |_| true)
            .await
            .unwrap();

        assert_eq!(stats.blocks_processed, 2);
        // Verify state accumulated: slot 0 should be 2
        let val = db.get_storage(&inc_addr, &B256::ZERO).unwrap().unwrap();
        assert_eq!(val, U256::from(2));
        // Caller nonce should be 2
        let caller_info = db.get_account(&caller).unwrap().unwrap();
        assert_eq!(caller_info.nonce, 2);
    }

    #[tokio::test]
    async fn pipeline_progress_reports_correct_data() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();
        let blocks: Vec<std::result::Result<_, std::convert::Infallible>> =
            (1..=3).map(|n| Ok(simple_block(n, vec![]))).collect();

        let mut progress_reports = Vec::new();
        run_pipeline(&db, &PipelineMode::Replay(spec.clone()), futures::stream::iter(blocks), |p| {
            progress_reports.push(p.clone());
            true
        })
        .await
        .unwrap();

        assert_eq!(progress_reports.len(), 3);
        assert_eq!(progress_reports[0].block_number, 1);
        assert_eq!(progress_reports[0].blocks_processed, 1);
        assert_eq!(progress_reports[0].tx_count, 0);
        assert_eq!(progress_reports[1].block_number, 2);
        assert_eq!(progress_reports[1].blocks_processed, 2);
        assert_eq!(progress_reports[2].block_number, 3);
        assert_eq!(progress_reports[2].blocks_processed, 3);
    }
}
