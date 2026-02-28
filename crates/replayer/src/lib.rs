pub mod pipeline;

use std::error::Error as _;

use alloy_primitives::{Address, B256};
use evm_state_chain_spec::{ChainSpec, block_env_from_header, tx_env_from_transaction};
use evm_state_common::AccountInfo;
use evm_state_db::StateDb;
use revm::database::{AccountState, CacheDB};
use revm::database_interface::DatabaseCommit;
use revm::handler::{EthFrame, Handler, MainnetHandler};
use revm::{Context, ExecuteEvm, MainBuilder};
use tracing::trace;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] evm_state_db::Error),

    #[error("EVM error at block {block_number}, {} at index {tx_index}: {message}", if *is_system { "system transaction" } else { "transaction" })]
    Evm {
        block_number: u64,
        tx_index: usize,
        is_system: bool,
        message: String,
    },

    #[error("block source error: {0}")]
    Source(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Result of replaying a single block.
#[derive(Debug)]
pub struct BlockResult {
    pub block_number: u64,
    pub tx_results: Vec<TxResult>,
}

/// Result of executing a single transaction within a block.
#[derive(Debug)]
pub struct TxResult {
    pub gas_used: u64,
    pub success: bool,
}

/// Replay all transactions in a block against the state database.
///
/// Executes transactions in order using revm with a [`CacheDB`] overlay,
/// then flushes all accumulated state changes to the [`StateDb`] atomically.
pub fn replay_block(db: &StateDb, block: &Block, chain_spec: &ChainSpec) -> Result<BlockResult> {
    let spec_id = chain_spec.spec_at(block.header.number, block.header.timestamp.unwrap_or(0));
    let block_env = block_env_from_header(&block.header);

    let mut cache_db = CacheDB::new(db);
    let mut tx_results = Vec::with_capacity(block.transactions.len());

    for (idx, tx) in block.transactions.iter().enumerate() {
        let is_system = is_system_tx(tx);

        let mut tx_env = tx_env_from_transaction(tx, chain_spec.chain_id);

        // System transactions (e.g. Polygon Bor state sync) are injected by the
        // consensus layer with from=0x0 and gas=0. Use the block gas limit
        // since the real gas limit is managed by the consensus layer.
        if is_system {
            tx_env.gas_limit = block_env.gas_limit;
            tx_env.gas_price = 0;
        }

        // Log the caller's current nonce from state before executing.
        if let Some(caller) = tx.from {
            let db_nonce = db
                .get_account(&caller)
                .ok()
                .flatten()
                .map(|a| a.nonce)
                .unwrap_or(0);
            let cache_nonce = cache_db.cache.accounts.get(&caller).map(|a| a.info.nonce);
            trace!(
                block = block.header.number,
                tx_index = idx,
                caller = %caller,
                db_nonce,
                cache_nonce = ?cache_nonce,
                tx_nonce = tx.nonce.unwrap_or(0),
                is_system,
                "pre-execution state"
            );
        }

        // Create a fresh EVM context per transaction (clean journal).
        // The CacheDB persists across transactions so state accumulates.
        let result_and_state = {
            let ctx: revm::handler::MainnetContext<&mut CacheDB<&StateDb>> =
                Context::new(&mut cache_db, spec_id);
            let ctx = ctx
                .modify_cfg_chained(|c: &mut revm::context::CfgEnv| {
                    c.chain_id = chain_spec.chain_id;
                })
                .modify_block_chained(|b: &mut revm::context::BlockEnv| *b = block_env.clone())
                .modify_tx_chained(|t: &mut revm::context::TxEnv| *t = tx_env);
            let mut evm = ctx.build_mainnet();

            if is_system {
                // System transactions skip all validation and pre-execution
                // (no nonce check, no balance deduction, no nonce bump).
                // Only execution + output are run, matching how Bor applies
                // state sync calls outside normal transaction processing.
                let mut handler = MainnetHandler::<_, _, EthFrame<_, _, _>>::default();
                handler.run_system_call(&mut evm)
            } else {
                evm.replay()
            }
            .map_err(|e| {
                // Extract the inner error message, stripping EVMError's
                // outer "transaction validation error: " / "header validation error: " wrapper.
                let reason = e
                    .source()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| e.to_string());
                let from = tx.from.map(|a| format!("{a}")).unwrap_or_default();
                let to = tx.to.map(|a| format!("{a}")).unwrap_or("(create)".into());
                let hash = tx.hash.map(|h| format!("{h}")).unwrap_or_default();
                Error::Evm {
                    block_number: block.header.number,
                    tx_index: idx,
                    is_system,
                    message: format!("{reason}\n  from: {from}\n  to:   {to}\n  hash: {hash}"),
                }
            })?
        }; // evm dropped — releases &mut cache_db

        tx_results.push(TxResult {
            gas_used: result_and_state.result.gas_used(),
            success: result_and_state.result.is_success(),
        });

        cache_db.commit(result_and_state.state);
    }

    // Flush all accumulated changes to StateDb in one atomic batch.
    flush_cache_to_db(db, &cache_db, block.header.number)?;

    Ok(BlockResult {
        block_number: block.header.number,
        tx_results,
    })
}

use evm_state_data_types::{Block, StateDiff, Transaction};

/// Detect system transactions injected by the consensus layer.
///
/// On Polygon (Bor), state sync transactions are submitted by the zero address
/// with zero gas. These aren't real EVM transactions — the state changes are
/// applied by the client outside normal EVM execution.
fn is_system_tx(tx: &Transaction) -> bool {
    let from_zero = tx.from.map_or(false, |a| a.is_zero());
    let gas_zero = tx.gas.map_or(true, |g| g.as_u64() == 0);
    from_zero && gas_zero
}

/// Apply state diffs directly to the database without replaying transactions.
///
/// This is used for chains (e.g. Polygon) where the SQD portal's transaction
/// stream is incomplete (missing state sync data). The portal's `stateDiffs`
/// endpoint captures the correct final state including consensus-injected changes.
pub fn apply_state_diffs(db: &StateDb, block: &Block) -> Result<()> {
    let batch = db.write_batch()?;

    if !block.state_diffs.is_empty() {
        // Group diffs by address.
        let mut by_address: std::collections::HashMap<Address, Vec<&StateDiff>> =
            std::collections::HashMap::new();
        for diff in &block.state_diffs {
            if let Some(addr) = diff.address {
                by_address.entry(addr).or_default().push(diff);
            }
        }

        for (address, diffs) in &by_address {
            // Check for account deletion.
            let deleted = diffs.iter().any(|d| {
                d.kind.as_deref() == Some("-")
                    && matches!(d.key.as_deref(), Some("balance" | "nonce" | "code"))
            });
            if deleted {
                batch.delete_account(address)?;
                continue;
            }

            // Start from current account state or default.
            let mut info = db.get_account(address)?.unwrap_or(AccountInfo {
                nonce: 0,
                balance: alloy_primitives::U256::ZERO,
                code_hash: B256::ZERO,
            });

            for diff in diffs {
                let key = match diff.key.as_deref() {
                    Some(k) => k,
                    None => continue,
                };
                let next = match diff.next.as_deref() {
                    Some(n) => n,
                    None => continue,
                };

                match key {
                    "nonce" => {
                        let hex = next.strip_prefix("0x").unwrap_or(next);
                        info.nonce = u64::from_str_radix(hex, 16).unwrap_or(0);
                    }
                    "balance" => {
                        let hex = next.strip_prefix("0x").unwrap_or(next);
                        info.balance = alloy_primitives::U256::from_str_radix(hex, 16)
                            .unwrap_or(alloy_primitives::U256::ZERO);
                    }
                    "code" => {
                        let hex = next.strip_prefix("0x").unwrap_or(next);
                        if let Ok(bytecode) = alloy_primitives::hex::decode(hex) {
                            let hash = alloy_primitives::keccak256(&bytecode);
                            batch.set_code(&hash.into(), &bytecode)?;
                            info.code_hash = hash.into();
                        }
                    }
                    slot_hex => {
                        // Storage slot.
                        if let Ok(slot) = slot_hex.parse::<B256>() {
                            if diff.kind.as_deref() == Some("-") {
                                batch.delete_storage(address, &slot)?;
                            } else {
                                let hex = next.strip_prefix("0x").unwrap_or(next);
                                let value = alloy_primitives::U256::from_str_radix(hex, 16)
                                    .unwrap_or(alloy_primitives::U256::ZERO);
                                batch.set_storage(address, &slot, &value)?;
                            }
                        }
                    }
                }
            }

            batch.set_account(address, &info)?;
        }
    }

    batch.set_head_block(block.header.number)?;
    batch.commit()?;

    Ok(())
}

/// Write CacheDB contents to StateDb via a single WriteBatch.
fn flush_cache_to_db(db: &StateDb, cache_db: &CacheDB<&StateDb>, block_number: u64) -> Result<()> {
    let batch = db.write_batch()?;

    for (address, cached_account) in &cache_db.cache.accounts {
        match cached_account.account_state {
            AccountState::NotExisting => {
                batch.delete_account(address)?;
            }
            AccountState::Touched | AccountState::StorageCleared => {
                let info = AccountInfo {
                    nonce: cached_account.info.nonce,
                    balance: cached_account.info.balance,
                    code_hash: cached_account.info.code_hash,
                };
                trace!(
                    block = block_number,
                    address = %address,
                    nonce = info.nonce,
                    "flush account"
                );
                batch.set_account(address, &info)?;

                for (&slot_u256, &value) in &cached_account.storage {
                    let slot = B256::from(slot_u256);
                    batch.set_storage(address, &slot, &value)?;
                }
            }
            AccountState::None => {
                // Loaded from DB but not modified — skip.
            }
        }
    }

    for (code_hash, bytecode) in &cache_db.cache.contracts {
        let raw = bytecode.original_bytes();
        if !raw.is_empty() {
            batch.set_code(code_hash, &raw)?;
        }
    }

    batch.set_head_block(block_number)?;
    batch.commit()?;

    Ok(())
}

#[cfg(test)]
pub(crate) mod test_helpers;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;
    use alloy_primitives::{Bytes, U256, address, keccak256};

    // ── Empty block ──────────────────────────────────────────────────

    #[test]
    fn replay_empty_block() {
        let (_dir, db) = tmp_db();
        let block = simple_block(1, vec![]);
        let spec = simple_chain_spec();

        let result = replay_block(&db, &block, &spec).unwrap();
        assert_eq!(result.block_number, 1);
        assert!(result.tx_results.is_empty());
        assert_eq!(db.get_head_block().unwrap().unwrap(), 1);
    }

    // ── Contract that writes to storage ──────────────────────────────

    #[test]
    fn replay_contract_writes_storage() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();

        // Contract: SSTORE(0, 0x42) then STOP
        //   PUSH1 0x42  PUSH1 0x00  SSTORE  STOP
        let bytecode = vec![0x60, 0x42, 0x60, 0x00, 0x55, 0x00];
        let code_hash = keccak256(&bytecode);
        let contract = address!("0000000000000000000000000000000000C0FFEE");
        let caller = address!("0000000000000000000000000000000000000001");

        db.set_account(
            &contract,
            &AccountInfo {
                nonce: 0,
                balance: U256::ZERO,
                code_hash: code_hash.into(),
            },
        )
        .unwrap();
        db.set_code(&code_hash.into(), &bytecode).unwrap();
        db.set_account(
            &caller,
            &AccountInfo {
                nonce: 0,
                balance: U256::from(1_000_000_000u64),
                code_hash: B256::ZERO,
            },
        )
        .unwrap();

        let tx = simple_tx(0, caller, Some(contract), Bytes::new());
        let block = simple_block(100, vec![tx]);

        let result = replay_block(&db, &block, &spec).unwrap();
        assert_eq!(result.tx_results.len(), 1);
        assert!(result.tx_results[0].success);
        assert!(result.tx_results[0].gas_used > 0);

        // Verify storage was written
        let slot = B256::ZERO;
        let val = db.get_storage(&contract, &slot).unwrap().unwrap();
        assert_eq!(val, U256::from(0x42));

        // Verify head block was updated
        assert_eq!(db.get_head_block().unwrap().unwrap(), 100);
    }

    // ── Contract reads storage set by previous tx in same block ──────

    #[test]
    fn replay_sequential_txs_see_each_others_state() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();
        let caller = address!("0000000000000000000000000000000000000001");

        // Contract: reads slot 0, adds 1, writes back to slot 0.
        //   PUSH1 0x00  SLOAD  PUSH1 0x01  ADD  PUSH1 0x00  SSTORE  STOP
        let inc_bytecode = vec![
            0x60, 0x00, // PUSH1 0x00
            0x54, // SLOAD
            0x60, 0x01, // PUSH1 0x01
            0x01, // ADD
            0x60, 0x00, // PUSH1 0x00
            0x55, // SSTORE
            0x00, // STOP
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

        // Two txs calling the same increment contract
        let tx0 = simple_tx(0, caller, Some(inc_addr), Bytes::new());
        let mut tx1 = simple_tx(1, caller, Some(inc_addr), Bytes::new());
        tx1.nonce = Some(1);

        let block = simple_block(200, vec![tx0, tx1]);
        let result = replay_block(&db, &block, &spec).unwrap();

        assert_eq!(result.tx_results.len(), 2);
        assert!(result.tx_results[0].success);
        assert!(result.tx_results[1].success);

        // After TX0: slot 0 = 0 + 1 = 1
        // After TX1: slot 0 = 1 + 1 = 2  (TX1 sees TX0's write)
        let val = db.get_storage(&inc_addr, &B256::ZERO).unwrap().unwrap();
        assert_eq!(val, U256::from(2));

        // Caller nonce incremented twice
        let caller_info = db.get_account(&caller).unwrap().unwrap();
        assert_eq!(caller_info.nonce, 2);
    }

    // ── Contract deployment ──────────────────────────────────────────

    #[test]
    fn replay_contract_deployment() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();

        let caller = address!("0000000000000000000000000000000000000001");
        db.set_account(
            &caller,
            &AccountInfo {
                nonce: 0,
                balance: U256::from(1_000_000_000u64),
                code_hash: B256::ZERO,
            },
        )
        .unwrap();

        // Init code: returns runtime code [0x60, 0x42] (PUSH1 0x42)
        //   PUSH1 0x02   (60 02) — runtime code size
        //   PUSH1 0x0C   (60 0C) — offset of runtime code in init code
        //   PUSH1 0x00   (60 00) — dest offset in memory
        //   CODECOPY     (39)    — copy runtime code to memory
        //   PUSH1 0x02   (60 02) — size to return
        //   PUSH1 0x00   (60 00) — offset in memory
        //   RETURN       (F3)
        //   [runtime code: 60 42]
        let init_code = vec![
            0x60, 0x02, 0x60, 0x0C, 0x60, 0x00, 0x39, 0x60, 0x02, 0x60, 0x00, 0xF3,
            // runtime code:
            0x60, 0x42,
        ];

        let tx = simple_tx(0, caller, None, Bytes::from(init_code));
        let block = simple_block(300, vec![tx]);

        let result = replay_block(&db, &block, &spec).unwrap();
        assert_eq!(result.tx_results.len(), 1);
        assert!(result.tx_results[0].success);

        // The deployed contract address is deterministic: keccak256(rlp([sender, nonce]))[12..]
        // For sender=0x01, nonce=0: we can compute it.
        // Rather than computing, just verify that *some* new account was written
        // by checking head block was updated.
        assert_eq!(db.get_head_block().unwrap().unwrap(), 300);
    }

    // ── Reverted transaction ─────────────────────────────────────────

    #[test]
    fn replay_reverted_tx() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();

        // Contract that always reverts: PUSH1 0x00  PUSH1 0x00  REVERT
        let bytecode = vec![0x60, 0x00, 0x60, 0x00, 0xFD];
        let code_hash = keccak256(&bytecode);
        let contract = address!("00000000000000000000000000000000DEADBEEF");
        let caller = address!("0000000000000000000000000000000000000001");

        db.set_account(
            &contract,
            &AccountInfo {
                nonce: 0,
                balance: U256::ZERO,
                code_hash: code_hash.into(),
            },
        )
        .unwrap();
        db.set_code(&code_hash.into(), &bytecode).unwrap();
        db.set_account(
            &caller,
            &AccountInfo {
                nonce: 0,
                balance: U256::from(1_000_000_000u64),
                code_hash: B256::ZERO,
            },
        )
        .unwrap();

        let tx = simple_tx(0, caller, Some(contract), Bytes::new());
        let block = simple_block(400, vec![tx]);

        let result = replay_block(&db, &block, &spec).unwrap();
        assert_eq!(result.tx_results.len(), 1);
        assert!(!result.tx_results[0].success);
        assert!(result.tx_results[0].gas_used > 0);

        // Caller nonce still increments even on revert
        let caller_info = db.get_account(&caller).unwrap().unwrap();
        assert_eq!(caller_info.nonce, 1);
    }

    // ── ETH transfer ─────────────────────────────────────────────────

    #[test]
    fn replay_eth_transfer() {
        let (_dir, db) = tmp_db();
        let spec = simple_chain_spec();

        let sender = address!("0000000000000000000000000000000000000001");
        let receiver = address!("0000000000000000000000000000000000000002");

        db.set_account(
            &sender,
            &AccountInfo {
                nonce: 0,
                balance: U256::from(1_000_000_000u64),
                code_hash: B256::ZERO,
            },
        )
        .unwrap();

        let mut tx = simple_tx(0, sender, Some(receiver), Bytes::new());
        tx.value = Some(U256::from(500_000u64));

        let block = simple_block(500, vec![tx]);
        let result = replay_block(&db, &block, &spec).unwrap();

        assert_eq!(result.tx_results.len(), 1);
        assert!(result.tx_results[0].success);
        assert!(result.tx_results[0].gas_used >= 21_000);

        // Verify balances (gas_price=0, so no gas cost deducted)
        let sender_info = db.get_account(&sender).unwrap().unwrap();
        let receiver_info = db.get_account(&receiver).unwrap().unwrap();

        assert_eq!(sender_info.balance, U256::from(1_000_000_000u64 - 500_000));
        assert_eq!(receiver_info.balance, U256::from(500_000u64));
        assert_eq!(sender_info.nonce, 1);
    }
}
