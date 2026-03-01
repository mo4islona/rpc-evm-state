use std::time::Instant;

use alloy_primitives::{Address, B256, U256};
use evm_state_common::AccountInfo;
use evm_state_db::{StateDb, WriteBatch};
use revm::bytecode::Bytecode;
use revm::database_interface::{Database, DatabaseRef};
use revm::state::AccountInfo as RevmAccountInfo;

use crate::definitions::*;

/// Transparent wrapper around [`StateDb`] that records read latencies
/// into Prometheus histograms.
///
/// Write operations delegate without instrumentation — writes are
/// already measured at the batch level by the replayer.
pub struct InstrumentedStateDb {
    inner: StateDb,
}

impl InstrumentedStateDb {
    pub fn new(db: StateDb) -> Self {
        Self { inner: db }
    }

    /// Access the inner StateDb for paths that don't need instrumentation.
    pub fn inner(&self) -> &StateDb {
        &self.inner
    }

    // ── Instrumented reads ──────────────────────────────────────

    pub fn get_account(&self, address: &Address) -> evm_state_db::Result<Option<AccountInfo>> {
        let start = Instant::now();
        let result = self.inner.get_account(address);
        DB_GET_ACCOUNT_DURATION.observe(start.elapsed().as_secs_f64());
        result
    }

    pub fn get_storage(
        &self,
        address: &Address,
        slot: &B256,
    ) -> evm_state_db::Result<Option<U256>> {
        let start = Instant::now();
        let result = self.inner.get_storage(address, slot);
        DB_GET_STORAGE_DURATION.observe(start.elapsed().as_secs_f64());
        result
    }

    pub fn get_code(&self, code_hash: &B256) -> evm_state_db::Result<Option<Vec<u8>>> {
        let start = Instant::now();
        let result = self.inner.get_code(code_hash);
        DB_GET_CODE_DURATION.observe(start.elapsed().as_secs_f64());
        result
    }

    // ── Passthrough (no instrumentation) ────────────────────────

    pub fn open(path: impl AsRef<std::path::Path>) -> evm_state_db::Result<Self> {
        StateDb::open(path).map(Self::new)
    }

    pub fn get_head_block(&self) -> evm_state_db::Result<Option<u64>> {
        self.inner.get_head_block()
    }

    pub fn set_head_block(&self, block_number: u64) -> evm_state_db::Result<()> {
        self.inner.set_head_block(block_number)
    }

    pub fn set_account(
        &self,
        address: &Address,
        info: &AccountInfo,
    ) -> evm_state_db::Result<()> {
        self.inner.set_account(address, info)
    }

    pub fn set_storage(
        &self,
        address: &Address,
        slot: &B256,
        value: &U256,
    ) -> evm_state_db::Result<()> {
        self.inner.set_storage(address, slot, value)
    }

    pub fn set_code(&self, code_hash: &B256, bytecode: &[u8]) -> evm_state_db::Result<()> {
        self.inner.set_code(code_hash, bytecode)
    }

    pub fn delete_account(&self, address: &Address) -> evm_state_db::Result<bool> {
        self.inner.delete_account(address)
    }

    pub fn delete_storage(&self, address: &Address, slot: &B256) -> evm_state_db::Result<bool> {
        self.inner.delete_storage(address, slot)
    }

    pub fn write_batch(&self) -> evm_state_db::Result<WriteBatch<'_>> {
        self.inner.write_batch()
    }

    pub fn iter_accounts(&self) -> evm_state_db::Result<Vec<(Address, AccountInfo)>> {
        self.inner.iter_accounts()
    }

    pub fn iter_storage(&self) -> evm_state_db::Result<Vec<(Address, B256, U256)>> {
        self.inner.iter_storage()
    }

    pub fn iter_code(&self) -> evm_state_db::Result<Vec<(B256, Vec<u8>)>> {
        self.inner.iter_code()
    }
}

// ── revm DatabaseRef ────────────────────────────────────────────────

impl DatabaseRef for InstrumentedStateDb {
    type Error = evm_state_db::Error;

    fn basic_ref(&self, address: Address) -> Result<Option<RevmAccountInfo>, Self::Error> {
        let account = self.get_account(&address)?;
        Ok(account.map(|info| RevmAccountInfo {
            balance: info.balance,
            nonce: info.nonce,
            code_hash: info.code_hash,
            code: None,
        }))
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        let code = self.get_code(&code_hash)?;
        Ok(match code {
            Some(bytes) => Bytecode::new_raw(bytes.into()),
            None => Bytecode::default(),
        })
    }

    fn storage_ref(&self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let slot = B256::from(index);
        Ok(self.get_storage(&address, &slot)?.unwrap_or(U256::ZERO))
    }

    fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

// ── revm Database ───────────────────────────────────────────────────

impl Database for InstrumentedStateDb {
    type Error = evm_state_db::Error;

    fn basic(&mut self, address: Address) -> Result<Option<RevmAccountInfo>, Self::Error> {
        self.basic_ref(address)
    }

    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        self.code_by_hash_ref(code_hash)
    }

    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        self.storage_ref(address, index)
    }

    fn block_hash(&mut self, number: u64) -> Result<B256, Self::Error> {
        self.block_hash_ref(number)
    }
}
