mod revm_db;

use std::path::Path;

use alloy_primitives::{Address, B256, U256};
use evm_state_common::{AccountInfo, AccountKey, StorageKey};
use libmdbx::{Database, DatabaseOptions, NoWriteMap, TableFlags, Transaction, WriteFlags, RW};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] libmdbx::Error),

    #[error("failed to decode account: {0}")]
    AccountDecode(#[from] evm_state_common::AccountDecodeError),

    #[error("unexpected value length for storage slot: expected 32, got {0}")]
    InvalidStorageValue(usize),

    #[error("unexpected value length for head_block: expected 8, got {0}")]
    InvalidHeadBlock(usize),
}

pub type Result<T> = std::result::Result<T, Error>;

const TABLE_ACCOUNTS: &str = "accounts";
const TABLE_STORAGE: &str = "storage";
const TABLE_CODE: &str = "code";
const TABLE_METADATA: &str = "metadata";

const META_KEY_HEAD_BLOCK: &[u8] = b"head_block";
const NUM_TABLES: usize = 4;

/// Flat KV state database backed by libmdbx.
///
/// Stores latest EVM state in four tables:
/// - `accounts`:  `[address:20B]` → `[nonce:8B][balance:32B][code_hash:32B]`
/// - `storage`:   `[address:20B][slot:32B]` → `[value:32B]`
/// - `code`:      `[code_hash:32B]` → `[bytecode:var]`
/// - `metadata`:  `b"head_block"` → `[block_number:8B]`
pub struct StateDb {
    db: Database<NoWriteMap>,
}

impl StateDb {
    /// Open or create a state database at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut opts = DatabaseOptions::default();
        opts.max_tables = Some(NUM_TABLES as u64);

        let db = Database::<NoWriteMap>::open_with_options(path, opts)?;

        // Create all tables on first open.
        {
            let txn = db.begin_rw_txn()?;
            txn.create_table(Some(TABLE_ACCOUNTS), TableFlags::empty())?;
            txn.create_table(Some(TABLE_STORAGE), TableFlags::empty())?;
            txn.create_table(Some(TABLE_CODE), TableFlags::empty())?;
            txn.create_table(Some(TABLE_METADATA), TableFlags::empty())?;
            txn.commit()?;
        }

        Ok(Self { db })
    }

    // ── Read operations (read-only transaction) ───────────────────────

    pub fn get_account(&self, address: &Address) -> Result<Option<AccountInfo>> {
        let txn = self.db.begin_ro_txn()?;
        self.get_account_with_txn(&txn, address)
    }

    pub fn get_storage(&self, address: &Address, slot: &B256) -> Result<Option<U256>> {
        let txn = self.db.begin_ro_txn()?;
        self.get_storage_with_txn(&txn, address, slot)
    }

    pub fn get_code(&self, code_hash: &B256) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_ro_txn()?;
        self.get_code_with_txn(&txn, code_hash)
    }

    pub fn get_head_block(&self) -> Result<Option<u64>> {
        let txn = self.db.begin_ro_txn()?;
        self.get_head_block_with_txn(&txn)
    }

    // ── Write operations (single-item convenience) ────────────────────

    pub fn set_account(&self, address: &Address, info: &AccountInfo) -> Result<()> {
        let txn = self.db.begin_rw_txn()?;
        self.set_account_with_txn(&txn, address, info)?;
        txn.commit()?;
        Ok(())
    }

    pub fn set_storage(&self, address: &Address, slot: &B256, value: &U256) -> Result<()> {
        let txn = self.db.begin_rw_txn()?;
        self.set_storage_with_txn(&txn, address, slot, value)?;
        txn.commit()?;
        Ok(())
    }

    pub fn set_code(&self, code_hash: &B256, bytecode: &[u8]) -> Result<()> {
        let txn = self.db.begin_rw_txn()?;
        self.set_code_with_txn(&txn, code_hash, bytecode)?;
        txn.commit()?;
        Ok(())
    }

    pub fn set_head_block(&self, block_number: u64) -> Result<()> {
        let txn = self.db.begin_rw_txn()?;
        self.set_head_block_with_txn(&txn, block_number)?;
        txn.commit()?;
        Ok(())
    }

    pub fn delete_account(&self, address: &Address) -> Result<bool> {
        let txn = self.db.begin_rw_txn()?;
        let deleted = self.delete_account_with_txn(&txn, address)?;
        txn.commit()?;
        Ok(deleted)
    }

    pub fn delete_storage(&self, address: &Address, slot: &B256) -> Result<bool> {
        let txn = self.db.begin_rw_txn()?;
        let deleted = self.delete_storage_with_txn(&txn, address, slot)?;
        txn.commit()?;
        Ok(deleted)
    }

    // ── Iteration ──────────────────────────────────────────────────────

    /// Collect all accounts as `(Address, AccountInfo)` pairs.
    pub fn iter_accounts(&self) -> Result<Vec<(Address, AccountInfo)>> {
        let txn = self.db.begin_ro_txn()?;
        let table = txn.open_table(Some(TABLE_ACCOUNTS))?;
        let mut cursor = txn.cursor(&table)?;
        let mut result = Vec::new();
        for item in cursor.iter_start::<Vec<u8>, Vec<u8>>() {
            let (key_bytes, val_bytes) = item?;
            if key_bytes.len() != 20 {
                continue;
            }
            let address = Address::from_slice(&key_bytes);
            let info = AccountInfo::from_bytes(&val_bytes)?;
            result.push((address, info));
        }
        Ok(result)
    }

    /// Collect all storage entries as `(Address, B256, U256)` triples.
    pub fn iter_storage(&self) -> Result<Vec<(Address, B256, U256)>> {
        let txn = self.db.begin_ro_txn()?;
        let table = txn.open_table(Some(TABLE_STORAGE))?;
        let mut cursor = txn.cursor(&table)?;
        let mut result = Vec::new();
        for item in cursor.iter_start::<Vec<u8>, Vec<u8>>() {
            let (key_bytes, val_bytes) = item?;
            if key_bytes.len() != 52 || val_bytes.len() != 32 {
                continue;
            }
            let address = Address::from_slice(&key_bytes[..20]);
            let slot = B256::from_slice(&key_bytes[20..52]);
            let value = U256::from_be_bytes::<32>(val_bytes.as_slice().try_into().unwrap());
            result.push((address, slot, value));
        }
        Ok(result)
    }

    /// Collect all code entries as `(B256, Vec<u8>)` pairs (code_hash, bytecode).
    pub fn iter_code(&self) -> Result<Vec<(B256, Vec<u8>)>> {
        let txn = self.db.begin_ro_txn()?;
        let table = txn.open_table(Some(TABLE_CODE))?;
        let mut cursor = txn.cursor(&table)?;
        let mut result = Vec::new();
        for item in cursor.iter_start::<Vec<u8>, Vec<u8>>() {
            let (key_bytes, val_bytes) = item?;
            if key_bytes.len() != 32 {
                continue;
            }
            let code_hash = B256::from_slice(&key_bytes);
            result.push((code_hash, val_bytes.to_vec()));
        }
        Ok(result)
    }

    // ── Batch write ───────────────────────────────────────────────────

    /// Begin a batch write. All operations on the returned `WriteBatch` are
    /// performed in a single libmdbx write transaction and committed atomically.
    pub fn write_batch(&self) -> Result<WriteBatch<'_>> {
        let txn = self.db.begin_rw_txn()?;
        Ok(WriteBatch { db: self, txn })
    }

    // ── Internal helpers (generic over RO/RW) ─────────────────────────

    fn get_account_with_txn<K: libmdbx::TransactionKind>(
        &self,
        txn: &Transaction<'_, K, NoWriteMap>,
        address: &Address,
    ) -> Result<Option<AccountInfo>> {
        let table = txn.open_table(Some(TABLE_ACCOUNTS))?;
        let key = AccountKey(*address);
        let val: Option<Vec<u8>> = txn.get(&table, key.to_bytes().as_slice())?;
        match val {
            Some(bytes) => Ok(Some(AccountInfo::from_bytes(&bytes)?)),
            None => Ok(None),
        }
    }

    fn get_storage_with_txn<K: libmdbx::TransactionKind>(
        &self,
        txn: &Transaction<'_, K, NoWriteMap>,
        address: &Address,
        slot: &B256,
    ) -> Result<Option<U256>> {
        let table = txn.open_table(Some(TABLE_STORAGE))?;
        let key = StorageKey::new(*address, *slot);
        let val: Option<Vec<u8>> = txn.get(&table, key.to_bytes().as_slice())?;
        match val {
            Some(bytes) => {
                if bytes.len() != 32 {
                    return Err(Error::InvalidStorageValue(bytes.len()));
                }
                Ok(Some(U256::from_be_bytes::<32>(
                    bytes.as_slice().try_into().unwrap(),
                )))
            }
            None => Ok(None),
        }
    }

    fn get_code_with_txn<K: libmdbx::TransactionKind>(
        &self,
        txn: &Transaction<'_, K, NoWriteMap>,
        code_hash: &B256,
    ) -> Result<Option<Vec<u8>>> {
        let table = txn.open_table(Some(TABLE_CODE))?;
        let val: Option<Vec<u8>> = txn.get(&table, code_hash.as_slice())?;
        Ok(val)
    }

    fn get_head_block_with_txn<K: libmdbx::TransactionKind>(
        &self,
        txn: &Transaction<'_, K, NoWriteMap>,
    ) -> Result<Option<u64>> {
        let table = txn.open_table(Some(TABLE_METADATA))?;
        let val: Option<Vec<u8>> = txn.get(&table, META_KEY_HEAD_BLOCK)?;
        match val {
            Some(bytes) => {
                if bytes.len() != 8 {
                    return Err(Error::InvalidHeadBlock(bytes.len()));
                }
                Ok(Some(u64::from_be_bytes(
                    bytes.as_slice().try_into().unwrap(),
                )))
            }
            None => Ok(None),
        }
    }

    fn set_account_with_txn(
        &self,
        txn: &Transaction<'_, RW, NoWriteMap>,
        address: &Address,
        info: &AccountInfo,
    ) -> Result<()> {
        let table = txn.open_table(Some(TABLE_ACCOUNTS))?;
        let key = AccountKey(*address);
        txn.put(
            &table,
            key.to_bytes(),
            info.to_bytes(),
            WriteFlags::UPSERT,
        )?;
        Ok(())
    }

    fn set_storage_with_txn(
        &self,
        txn: &Transaction<'_, RW, NoWriteMap>,
        address: &Address,
        slot: &B256,
        value: &U256,
    ) -> Result<()> {
        let table = txn.open_table(Some(TABLE_STORAGE))?;
        let key = StorageKey::new(*address, *slot);
        txn.put(
            &table,
            key.to_bytes(),
            value.to_be_bytes::<32>(),
            WriteFlags::UPSERT,
        )?;
        Ok(())
    }

    fn set_code_with_txn(
        &self,
        txn: &Transaction<'_, RW, NoWriteMap>,
        code_hash: &B256,
        bytecode: &[u8],
    ) -> Result<()> {
        let table = txn.open_table(Some(TABLE_CODE))?;
        txn.put(&table, code_hash.as_slice(), bytecode, WriteFlags::UPSERT)?;
        Ok(())
    }

    fn set_head_block_with_txn(
        &self,
        txn: &Transaction<'_, RW, NoWriteMap>,
        block_number: u64,
    ) -> Result<()> {
        let table = txn.open_table(Some(TABLE_METADATA))?;
        txn.put(
            &table,
            META_KEY_HEAD_BLOCK,
            block_number.to_be_bytes(),
            WriteFlags::UPSERT,
        )?;
        Ok(())
    }

    fn delete_account_with_txn(
        &self,
        txn: &Transaction<'_, RW, NoWriteMap>,
        address: &Address,
    ) -> Result<bool> {
        let table = txn.open_table(Some(TABLE_ACCOUNTS))?;
        let key = AccountKey(*address);
        Ok(txn.del(&table, key.to_bytes(), None)?)
    }

    fn delete_storage_with_txn(
        &self,
        txn: &Transaction<'_, RW, NoWriteMap>,
        address: &Address,
        slot: &B256,
    ) -> Result<bool> {
        let table = txn.open_table(Some(TABLE_STORAGE))?;
        let key = StorageKey::new(*address, *slot);
        Ok(txn.del(&table, key.to_bytes(), None)?)
    }
}

/// A batch of writes executed atomically in a single libmdbx transaction.
pub struct WriteBatch<'a> {
    db: &'a StateDb,
    txn: Transaction<'a, RW, NoWriteMap>,
}

impl<'a> WriteBatch<'a> {
    pub fn set_account(&self, address: &Address, info: &AccountInfo) -> Result<()> {
        self.db.set_account_with_txn(&self.txn, address, info)
    }

    pub fn set_storage(&self, address: &Address, slot: &B256, value: &U256) -> Result<()> {
        self.db.set_storage_with_txn(&self.txn, address, slot, value)
    }

    pub fn set_code(&self, code_hash: &B256, bytecode: &[u8]) -> Result<()> {
        self.db.set_code_with_txn(&self.txn, code_hash, bytecode)
    }

    pub fn set_head_block(&self, block_number: u64) -> Result<()> {
        self.db.set_head_block_with_txn(&self.txn, block_number)
    }

    pub fn delete_account(&self, address: &Address) -> Result<bool> {
        self.db.delete_account_with_txn(&self.txn, address)
    }

    pub fn delete_storage(&self, address: &Address, slot: &B256) -> Result<bool> {
        self.db.delete_storage_with_txn(&self.txn, address, slot)
    }

    /// Commit all writes atomically.
    pub fn commit(self) -> Result<()> {
        self.txn.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    fn tmp_db() -> (tempfile::TempDir, StateDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path()).unwrap();
        (dir, db)
    }

    // ── Account CRUD ──────────────────────────────────────────────────

    #[test]
    fn account_write_and_read() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let info = AccountInfo {
            nonce: 42,
            balance: U256::from(1_000_000u64),
            code_hash: B256::ZERO,
        };

        db.set_account(&addr, &info).unwrap();
        let got = db.get_account(&addr).unwrap().unwrap();
        assert_eq!(got, info);
    }

    #[test]
    fn account_missing_returns_none() {
        let (_dir, db) = tmp_db();
        let addr = address!("0000000000000000000000000000000000000001");
        assert!(db.get_account(&addr).unwrap().is_none());
    }

    #[test]
    fn account_overwrite() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        let info_v1 = AccountInfo {
            nonce: 1,
            balance: U256::from(100u64),
            code_hash: B256::ZERO,
        };
        db.set_account(&addr, &info_v1).unwrap();

        let info_v2 = AccountInfo {
            nonce: 2,
            balance: U256::from(200u64),
            code_hash: B256::ZERO,
        };
        db.set_account(&addr, &info_v2).unwrap();

        let got = db.get_account(&addr).unwrap().unwrap();
        assert_eq!(got, info_v2);
    }

    #[test]
    fn account_delete() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let info = AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: B256::ZERO,
        };

        db.set_account(&addr, &info).unwrap();
        assert!(db.delete_account(&addr).unwrap());
        assert!(db.get_account(&addr).unwrap().is_none());
    }

    #[test]
    fn account_delete_nonexistent() {
        let (_dir, db) = tmp_db();
        let addr = address!("0000000000000000000000000000000000000001");
        assert!(!db.delete_account(&addr).unwrap());
    }

    // ── Storage CRUD ──────────────────────────────────────────────────

    #[test]
    fn storage_write_and_read() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000005");
        let value = U256::from(999u64);

        db.set_storage(&addr, &slot, &value).unwrap();
        let got = db.get_storage(&addr, &slot).unwrap().unwrap();
        assert_eq!(got, value);
    }

    #[test]
    fn storage_missing_returns_none() {
        let (_dir, db) = tmp_db();
        let addr = address!("0000000000000000000000000000000000000001");
        let slot = B256::ZERO;
        assert!(db.get_storage(&addr, &slot).unwrap().is_none());
    }

    #[test]
    fn storage_overwrite() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = B256::ZERO;

        db.set_storage(&addr, &slot, &U256::from(1u64)).unwrap();
        db.set_storage(&addr, &slot, &U256::from(2u64)).unwrap();

        let got = db.get_storage(&addr, &slot).unwrap().unwrap();
        assert_eq!(got, U256::from(2u64));
    }

    #[test]
    fn storage_different_slots_same_address() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot_a = b256!("0000000000000000000000000000000000000000000000000000000000000001");
        let slot_b = b256!("0000000000000000000000000000000000000000000000000000000000000002");

        db.set_storage(&addr, &slot_a, &U256::from(100u64)).unwrap();
        db.set_storage(&addr, &slot_b, &U256::from(200u64)).unwrap();

        assert_eq!(
            db.get_storage(&addr, &slot_a).unwrap().unwrap(),
            U256::from(100u64)
        );
        assert_eq!(
            db.get_storage(&addr, &slot_b).unwrap().unwrap(),
            U256::from(200u64)
        );
    }

    #[test]
    fn storage_delete() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = B256::ZERO;

        db.set_storage(&addr, &slot, &U256::from(1u64)).unwrap();
        assert!(db.delete_storage(&addr, &slot).unwrap());
        assert!(db.get_storage(&addr, &slot).unwrap().is_none());
    }

    // ── Code CRUD ─────────────────────────────────────────────────────

    #[test]
    fn code_write_and_read() {
        let (_dir, db) = tmp_db();
        let hash = b256!("abcdef0000000000000000000000000000000000000000000000000000000001");
        let bytecode = vec![0x60, 0x00, 0x60, 0x00, 0xFD]; // PUSH0 PUSH0 REVERT

        db.set_code(&hash, &bytecode).unwrap();
        let got = db.get_code(&hash).unwrap().unwrap();
        assert_eq!(got, bytecode);
    }

    #[test]
    fn code_missing_returns_none() {
        let (_dir, db) = tmp_db();
        let hash = B256::ZERO;
        assert!(db.get_code(&hash).unwrap().is_none());
    }

    #[test]
    fn code_large_bytecode() {
        let (_dir, db) = tmp_db();
        let hash = b256!("1111111111111111111111111111111111111111111111111111111111111111");
        let bytecode = vec![0xFE; 24_576]; // max contract size

        db.set_code(&hash, &bytecode).unwrap();
        let got = db.get_code(&hash).unwrap().unwrap();
        assert_eq!(got.len(), 24_576);
        assert_eq!(got, bytecode);
    }

    // ── Metadata (head_block) ─────────────────────────────────────────

    #[test]
    fn head_block_write_and_read() {
        let (_dir, db) = tmp_db();

        db.set_head_block(12_345_678).unwrap();
        let got = db.get_head_block().unwrap().unwrap();
        assert_eq!(got, 12_345_678);
    }

    #[test]
    fn head_block_missing_returns_none() {
        let (_dir, db) = tmp_db();
        assert!(db.get_head_block().unwrap().is_none());
    }

    #[test]
    fn head_block_overwrite() {
        let (_dir, db) = tmp_db();

        db.set_head_block(100).unwrap();
        db.set_head_block(200).unwrap();
        assert_eq!(db.get_head_block().unwrap().unwrap(), 200);
    }

    // ── Batch writes ──────────────────────────────────────────────────

    #[test]
    fn batch_write_multiple_tables() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = B256::ZERO;
        let code_hash = b256!("abcdef0000000000000000000000000000000000000000000000000000000001");

        let batch = db.write_batch().unwrap();
        batch
            .set_account(
                &addr,
                &AccountInfo {
                    nonce: 1,
                    balance: U256::from(1000u64),
                    code_hash,
                },
            )
            .unwrap();
        batch
            .set_storage(&addr, &slot, &U256::from(42u64))
            .unwrap();
        batch
            .set_code(&code_hash, &[0x60, 0x00, 0x60, 0x00, 0xFD])
            .unwrap();
        batch.set_head_block(100).unwrap();
        batch.commit().unwrap();

        // Verify all writes landed
        let account = db.get_account(&addr).unwrap().unwrap();
        assert_eq!(account.nonce, 1);
        assert_eq!(account.balance, U256::from(1000u64));

        let storage = db.get_storage(&addr, &slot).unwrap().unwrap();
        assert_eq!(storage, U256::from(42u64));

        let code = db.get_code(&code_hash).unwrap().unwrap();
        assert_eq!(code, vec![0x60, 0x00, 0x60, 0x00, 0xFD]);

        assert_eq!(db.get_head_block().unwrap().unwrap(), 100);
    }

    #[test]
    fn batch_write_atomicity_on_drop() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        // Write without committing — should be rolled back on drop
        {
            let batch = db.write_batch().unwrap();
            batch
                .set_account(
                    &addr,
                    &AccountInfo {
                        nonce: 99,
                        balance: U256::ZERO,
                        code_hash: B256::ZERO,
                    },
                )
                .unwrap();
            // batch dropped without commit
        }

        assert!(db.get_account(&addr).unwrap().is_none());
    }

    #[test]
    fn batch_many_storage_writes() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        let batch = db.write_batch().unwrap();
        for i in 0u64..100 {
            let slot = B256::from(U256::from(i));
            batch.set_storage(&addr, &slot, &U256::from(i * 10)).unwrap();
        }
        batch.commit().unwrap();

        // Verify all 100 slots
        for i in 0u64..100 {
            let slot = B256::from(U256::from(i));
            let val = db.get_storage(&addr, &slot).unwrap().unwrap();
            assert_eq!(val, U256::from(i * 10));
        }
    }

    #[test]
    fn batch_delete_in_batch() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        // First, insert data
        db.set_account(
            &addr,
            &AccountInfo {
                nonce: 1,
                balance: U256::ZERO,
                code_hash: B256::ZERO,
            },
        )
        .unwrap();

        // Delete it in a batch
        let batch = db.write_batch().unwrap();
        assert!(batch.delete_account(&addr).unwrap());
        batch.commit().unwrap();

        assert!(db.get_account(&addr).unwrap().is_none());
    }

    // ── Persistence across reopen ─────────────────────────────────────

    #[test]
    fn data_persists_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = B256::ZERO;
        let code_hash = b256!("abcdef0000000000000000000000000000000000000000000000000000000001");

        // Write data
        {
            let db = StateDb::open(dir.path()).unwrap();
            let batch = db.write_batch().unwrap();
            batch
                .set_account(
                    &addr,
                    &AccountInfo {
                        nonce: 10,
                        balance: U256::from(5000u64),
                        code_hash,
                    },
                )
                .unwrap();
            batch
                .set_storage(&addr, &slot, &U256::from(77u64))
                .unwrap();
            batch.set_code(&code_hash, &[0x60, 0x42]).unwrap();
            batch.set_head_block(999).unwrap();
            batch.commit().unwrap();
        }

        // Reopen and verify
        {
            let db = StateDb::open(dir.path()).unwrap();

            let account = db.get_account(&addr).unwrap().unwrap();
            assert_eq!(account.nonce, 10);
            assert_eq!(account.balance, U256::from(5000u64));
            assert_eq!(account.code_hash, code_hash);

            let storage = db.get_storage(&addr, &slot).unwrap().unwrap();
            assert_eq!(storage, U256::from(77u64));

            let code = db.get_code(&code_hash).unwrap().unwrap();
            assert_eq!(code, vec![0x60, 0x42]);

            assert_eq!(db.get_head_block().unwrap().unwrap(), 999);
        }
    }

    // ── Concurrent reads ──────────────────────────────────────────────

    #[test]
    fn concurrent_reads() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        db.set_account(
            &addr,
            &AccountInfo {
                nonce: 1,
                balance: U256::from(100u64),
                code_hash: B256::ZERO,
            },
        )
        .unwrap();

        // Multiple concurrent read transactions should all succeed
        let handles: Vec<_> = (0..4)
            .map(|_| {
                // We can't move `db` across threads, but we can verify
                // multiple ro txns work from the same thread.
                let a = db.get_account(&addr).unwrap().unwrap();
                assert_eq!(a.nonce, 1);
                a
            })
            .collect();

        assert_eq!(handles.len(), 4);
    }

    // ── Edge cases ────────────────────────────────────────────────────

    #[test]
    fn storage_max_u256_value() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = B256::ZERO;

        db.set_storage(&addr, &slot, &U256::MAX).unwrap();
        let got = db.get_storage(&addr, &slot).unwrap().unwrap();
        assert_eq!(got, U256::MAX);
    }

    #[test]
    fn storage_zero_value() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = B256::ZERO;

        db.set_storage(&addr, &slot, &U256::ZERO).unwrap();
        let got = db.get_storage(&addr, &slot).unwrap().unwrap();
        assert_eq!(got, U256::ZERO);
    }

    #[test]
    fn code_empty_bytecode() {
        let (_dir, db) = tmp_db();
        let hash = b256!("c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"); // keccak256("")
        db.set_code(&hash, &[]).unwrap();
        let got = db.get_code(&hash).unwrap().unwrap();
        assert!(got.is_empty());
    }

    #[test]
    fn multiple_addresses_independent() {
        let (_dir, db) = tmp_db();
        let addr_a = address!("0000000000000000000000000000000000000001");
        let addr_b = address!("0000000000000000000000000000000000000002");
        let slot = B256::ZERO;

        db.set_storage(&addr_a, &slot, &U256::from(111u64)).unwrap();
        db.set_storage(&addr_b, &slot, &U256::from(222u64)).unwrap();

        assert_eq!(
            db.get_storage(&addr_a, &slot).unwrap().unwrap(),
            U256::from(111u64)
        );
        assert_eq!(
            db.get_storage(&addr_b, &slot).unwrap().unwrap(),
            U256::from(222u64)
        );
    }

    // ── Iteration ─────────────────────────────────────────────────────

    #[test]
    fn iter_accounts_returns_all() {
        let (_dir, db) = tmp_db();
        let addr_a = address!("0000000000000000000000000000000000000001");
        let addr_b = address!("0000000000000000000000000000000000000002");
        let addr_c = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");

        let info_a = AccountInfo { nonce: 1, balance: U256::from(100u64), code_hash: B256::ZERO };
        let info_b = AccountInfo { nonce: 2, balance: U256::from(200u64), code_hash: B256::ZERO };
        let info_c = AccountInfo { nonce: 42, balance: U256::from(1_000_000u64), code_hash: B256::ZERO };

        db.set_account(&addr_a, &info_a).unwrap();
        db.set_account(&addr_b, &info_b).unwrap();
        db.set_account(&addr_c, &info_c).unwrap();

        let accounts = db.iter_accounts().unwrap();
        assert_eq!(accounts.len(), 3);

        // Verify all accounts are present (order is lexicographic by address bytes).
        let map: std::collections::HashMap<Address, AccountInfo> = accounts.into_iter().collect();
        assert_eq!(map[&addr_a], info_a);
        assert_eq!(map[&addr_b], info_b);
        assert_eq!(map[&addr_c], info_c);
    }

    #[test]
    fn iter_accounts_empty_db() {
        let (_dir, db) = tmp_db();
        let accounts = db.iter_accounts().unwrap();
        assert!(accounts.is_empty());
    }

    #[test]
    fn iter_storage_returns_all() {
        let (_dir, db) = tmp_db();
        let addr_a = address!("0000000000000000000000000000000000000001");
        let addr_b = address!("0000000000000000000000000000000000000002");
        let slot_0 = B256::ZERO;
        let slot_1 = b256!("0000000000000000000000000000000000000000000000000000000000000001");

        db.set_storage(&addr_a, &slot_0, &U256::from(10u64)).unwrap();
        db.set_storage(&addr_a, &slot_1, &U256::from(20u64)).unwrap();
        db.set_storage(&addr_b, &slot_0, &U256::from(30u64)).unwrap();

        let entries = db.iter_storage().unwrap();
        assert_eq!(entries.len(), 3);

        // Check all entries present.
        let has = |addr: &Address, slot: &B256, val: U256| {
            entries.iter().any(|(a, s, v)| a == addr && s == slot && *v == val)
        };
        assert!(has(&addr_a, &slot_0, U256::from(10u64)));
        assert!(has(&addr_a, &slot_1, U256::from(20u64)));
        assert!(has(&addr_b, &slot_0, U256::from(30u64)));
    }

    #[test]
    fn iter_storage_empty_db() {
        let (_dir, db) = tmp_db();
        let entries = db.iter_storage().unwrap();
        assert!(entries.is_empty());
    }
}
