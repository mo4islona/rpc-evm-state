use alloy_primitives::{Address, Bytes, B256};
use evm_state_chain_spec::ChainSpec;
use evm_state_data_types::{Block, BlockHeader, HexU64, Transaction};
use evm_state_db::StateDb;

pub(crate) fn tmp_db() -> (tempfile::TempDir, StateDb) {
    let dir = tempfile::tempdir().unwrap();
    let db = StateDb::open(dir.path()).unwrap();
    (dir, db)
}

pub(crate) fn simple_chain_spec() -> ChainSpec {
    ChainSpec {
        chain_id: 1,
        name: "test",
        hardforks: vec![evm_state_chain_spec::HardforkActivation {
            condition: evm_state_chain_spec::HardforkCondition::Block(0),
            spec_id: revm::primitives::hardfork::SpecId::SHANGHAI,
        }],
    }
}

pub(crate) fn simple_block(number: u64, txs: Vec<Transaction>) -> Block {
    Block {
        header: BlockHeader {
            number,
            hash: None,
            parent_hash: None,
            timestamp: Some(1_000_000),
            nonce: None,
            sha3_uncles: None,
            transactions_root: None,
            state_root: None,
            receipts_root: None,
            mix_hash: Some(B256::ZERO),
            miner: Some(Address::ZERO),
            difficulty: None,
            extra_data: None,
            size: None,
            gas_limit: Some(HexU64::from(30_000_000u64)),
            gas_used: None,
            base_fee_per_gas: Some(HexU64::from(0u64)),
            l1_block_number: None,
        },
        transactions: txs,
    }
}

pub(crate) fn simple_tx(
    index: u32,
    from: Address,
    to: Option<Address>,
    input: Bytes,
) -> Transaction {
    Transaction {
        transaction_index: index,
        hash: None,
        from: Some(from),
        to,
        input: Some(input),
        value: Some(HexU64::from(0u64)),
        nonce: Some(0),
        gas: Some(HexU64::from(1_000_000u64)),
        gas_price: Some(HexU64::from(0u64)),
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        y_parity: None,
        chain_id: Some(1),
        gas_used: None,
        cumulative_gas_used: None,
        effective_gas_price: None,
        contract_address: None,
        tx_type: Some(0),
        status: None,
        sighash: None,
    }
}
