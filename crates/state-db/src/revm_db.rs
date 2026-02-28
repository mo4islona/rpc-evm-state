use alloy_primitives::{Address, B256, U256};
use revm::bytecode::Bytecode;
use revm::database_interface::{DBErrorMarker, Database, DatabaseRef};
use revm::state::AccountInfo as RevmAccountInfo;

use crate::{Error, StateDb};

impl DBErrorMarker for Error {}

impl DatabaseRef for StateDb {
    type Error = Error;

    fn basic_ref(&self, address: Address) -> Result<Option<RevmAccountInfo>, Self::Error> {
        let account = self.get_account(&address)?;
        Ok(account.map(|info| RevmAccountInfo {
            balance: info.balance,
            nonce: info.nonce,
            code_hash: info.code_hash,
            code: None, // loaded via code_by_hash when needed
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
        // We don't store block hashes — return zero.
        Ok(B256::ZERO)
    }
}

impl Database for StateDb {
    type Error = Error;

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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, Bytes};
    use evm_state_common::AccountInfo;
    use revm::database_interface::DatabaseRef;
    use revm::primitives::hardfork::SpecId;
    use revm::{Context, ExecuteEvm, MainBuilder};

    fn tmp_db() -> (tempfile::TempDir, StateDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = StateDb::open(dir.path()).unwrap();
        (dir, db)
    }

    // ── DatabaseRef basic operations ──────────────────────────────────

    #[test]
    fn basic_ref_returns_account_info() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let info = AccountInfo {
            nonce: 42,
            balance: U256::from(1_000_000u64),
            code_hash: B256::ZERO,
        };
        db.set_account(&addr, &info).unwrap();

        let revm_info = db.basic_ref(addr).unwrap().unwrap();
        assert_eq!(revm_info.nonce, 42);
        assert_eq!(revm_info.balance, U256::from(1_000_000u64));
        assert_eq!(revm_info.code_hash, B256::ZERO);
        assert!(revm_info.code.is_none());
    }

    #[test]
    fn basic_ref_missing_returns_none() {
        let (_dir, db) = tmp_db();
        let addr = address!("0000000000000000000000000000000000000001");
        assert!(db.basic_ref(addr).unwrap().is_none());
    }

    #[test]
    fn storage_ref_returns_value() {
        let (_dir, db) = tmp_db();
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000005");
        let value = U256::from(999u64);

        db.set_storage(&addr, &slot, &value).unwrap();

        let got = db.storage_ref(addr, U256::from_be_bytes(slot.0)).unwrap();
        assert_eq!(got, value);
    }

    #[test]
    fn storage_ref_missing_returns_zero() {
        let (_dir, db) = tmp_db();
        let addr = address!("0000000000000000000000000000000000000001");
        let got = db.storage_ref(addr, U256::ZERO).unwrap();
        assert_eq!(got, U256::ZERO);
    }

    #[test]
    fn code_by_hash_ref_returns_bytecode() {
        let (_dir, db) = tmp_db();
        let hash = b256!("abcdef0000000000000000000000000000000000000000000000000000000001");
        let raw = vec![0x60, 0x42, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3];

        db.set_code(&hash, &raw).unwrap();

        let bytecode = db.code_by_hash_ref(hash).unwrap();
        assert_eq!(bytecode.original_bytes(), Bytes::from(raw));
    }

    #[test]
    fn code_by_hash_ref_missing_returns_default() {
        let (_dir, db) = tmp_db();
        let hash = b256!("0000000000000000000000000000000000000000000000000000000000000099");
        let bytecode = db.code_by_hash_ref(hash).unwrap();
        assert!(bytecode.is_empty());
    }

    #[test]
    fn block_hash_ref_returns_zero() {
        let (_dir, db) = tmp_db();
        assert_eq!(db.block_hash_ref(12345).unwrap(), B256::ZERO);
    }

    // ── Execute via revm ──────────────────────────────────────────────

    #[test]
    fn execute_simple_return_contract() {
        let (_dir, mut db) = tmp_db();

        // Contract that returns 0x42:
        //   PUSH1 0x42  PUSH1 0x00  MSTORE  PUSH1 0x20  PUSH1 0x00  RETURN
        let bytecode = vec![0x60, 0x42, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3];
        let code_hash = b256!("abcdef0000000000000000000000000000000000000000000000000000000001");
        let contract_addr = address!("0000000000000000000000000000000000C0FFEE");
        let caller = address!("0000000000000000000000000000000000000001");

        db.set_account(
            &contract_addr,
            &AccountInfo { nonce: 0, balance: U256::ZERO, code_hash },
        ).unwrap();
        db.set_code(&code_hash, &bytecode).unwrap();
        db.set_account(
            &caller,
            &AccountInfo { nonce: 0, balance: U256::from(1_000_000_000u64), code_hash: B256::ZERO },
        ).unwrap();

        let ctx: revm::handler::MainnetContext<&mut StateDb> = Context::new(&mut db, SpecId::CANCUN)
            .modify_block_chained(|block: &mut revm::context::BlockEnv| {
                block.number = 1;
                block.gas_limit = 30_000_000;
            })
            .modify_tx_chained(|tx: &mut revm::context::TxEnv| {
                tx.caller = caller;
                tx.gas_limit = 100_000;
                tx.kind = alloy_primitives::TxKind::Call(contract_addr);
            });

        let mut evm = ctx.build_mainnet();
        let result = evm.replay().unwrap();

        let output = result.result.output().unwrap();
        let returned_value = U256::from_be_slice(output);
        assert_eq!(returned_value, U256::from(0x42));
    }

    #[test]
    fn execute_reads_storage_from_db() {
        let (_dir, mut db) = tmp_db();

        // Contract that returns storage slot 0:
        //   PUSH1 0x00  SLOAD  PUSH1 0x00  MSTORE  PUSH1 0x20  PUSH1 0x00  RETURN
        let bytecode = vec![0x60, 0x00, 0x54, 0x60, 0x00, 0x52, 0x60, 0x20, 0x60, 0x00, 0xF3];
        let code_hash = b256!("1111111100000000000000000000000000000000000000000000000000000001");
        let contract_addr = address!("00000000000000000000000000000000DEADBEEF");
        let caller = address!("0000000000000000000000000000000000000001");

        db.set_account(
            &contract_addr,
            &AccountInfo { nonce: 0, balance: U256::ZERO, code_hash },
        ).unwrap();
        db.set_code(&code_hash, &bytecode).unwrap();
        db.set_storage(&contract_addr, &B256::ZERO, &U256::from(0xCAFEu64)).unwrap();
        db.set_account(
            &caller,
            &AccountInfo { nonce: 0, balance: U256::from(1_000_000_000u64), code_hash: B256::ZERO },
        ).unwrap();

        let ctx: revm::handler::MainnetContext<&mut StateDb> = Context::new(&mut db, SpecId::CANCUN)
            .modify_block_chained(|block: &mut revm::context::BlockEnv| {
                block.number = 1;
                block.gas_limit = 30_000_000;
            })
            .modify_tx_chained(|tx: &mut revm::context::TxEnv| {
                tx.caller = caller;
                tx.gas_limit = 100_000;
                tx.kind = alloy_primitives::TxKind::Call(contract_addr);
            });

        let mut evm = ctx.build_mainnet();
        let result = evm.replay().unwrap();

        let output = result.result.output().unwrap();
        let returned_value = U256::from_be_slice(output);
        assert_eq!(returned_value, U256::from(0xCAFEu64));
    }
}
