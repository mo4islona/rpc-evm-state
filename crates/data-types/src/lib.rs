mod hex_num;

pub use hex_num::HexU64;

use alloy_primitives::{Address, Bytes, B256};
use serde::{Deserialize, Serialize};

/// A block as returned by the SQD Network EVM streaming API.
///
/// Field presence depends on the `fields` selector in the request query.
/// For the replayer, we request all fields needed for EVM execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Block {
    pub header: BlockHeader,

    #[serde(default)]
    pub transactions: Vec<Transaction>,
}

/// Block header from SQD Network.
///
/// Numeric fields that the API returns as hex strings use [`HexU64`].
/// Fields that are always present when requested are non-optional.
/// Fields that may be absent (not requested or not available) are `Option`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockHeader {
    pub number: u64,

    #[serde(default)]
    pub hash: Option<B256>,

    #[serde(default)]
    pub parent_hash: Option<B256>,

    #[serde(default)]
    pub timestamp: Option<u64>,

    /// Block nonce — 8-byte hex string (e.g. "0x0000000000000000").
    #[serde(default)]
    pub nonce: Option<String>,

    #[serde(default)]
    pub sha3_uncles: Option<B256>,

    #[serde(default)]
    pub transactions_root: Option<B256>,

    #[serde(default)]
    pub state_root: Option<B256>,

    #[serde(default)]
    pub receipts_root: Option<B256>,

    #[serde(default)]
    pub mix_hash: Option<B256>,

    #[serde(default)]
    pub miner: Option<Address>,

    #[serde(default)]
    pub difficulty: Option<HexU64>,

    #[serde(default)]
    pub extra_data: Option<Bytes>,

    #[serde(default)]
    pub size: Option<u64>,

    #[serde(default)]
    pub gas_limit: Option<HexU64>,

    #[serde(default)]
    pub gas_used: Option<HexU64>,

    #[serde(default)]
    pub base_fee_per_gas: Option<HexU64>,

    // L2-specific
    #[serde(default)]
    pub l1_block_number: Option<u64>,
}

/// Transaction from SQD Network.
///
/// Covers legacy (type 0), EIP-2930 (type 1), and EIP-1559 (type 2) transactions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transaction {
    pub transaction_index: u32,

    #[serde(default)]
    pub hash: Option<B256>,

    #[serde(default)]
    pub from: Option<Address>,

    #[serde(default)]
    pub to: Option<Address>,

    #[serde(default)]
    pub input: Option<Bytes>,

    #[serde(default)]
    pub value: Option<HexU64>,

    #[serde(default)]
    pub nonce: Option<u64>,

    #[serde(default)]
    pub gas: Option<HexU64>,

    #[serde(default)]
    pub gas_price: Option<HexU64>,

    // EIP-1559 fields
    #[serde(default)]
    pub max_fee_per_gas: Option<HexU64>,

    #[serde(default)]
    pub max_priority_fee_per_gas: Option<HexU64>,

    // Signature
    #[serde(default)]
    pub y_parity: Option<u64>,

    #[serde(default)]
    pub chain_id: Option<u64>,

    // Receipt fields
    #[serde(default)]
    pub gas_used: Option<HexU64>,

    #[serde(default)]
    pub cumulative_gas_used: Option<HexU64>,

    #[serde(default)]
    pub effective_gas_price: Option<HexU64>,

    #[serde(default)]
    pub contract_address: Option<Address>,

    /// Transaction type: 0 = legacy, 1 = EIP-2930, 2 = EIP-1559.
    #[serde(default, rename = "type")]
    pub tx_type: Option<u8>,

    /// 1 = success, 0 = failure.
    #[serde(default)]
    pub status: Option<u8>,

    #[serde(default)]
    pub sighash: Option<Bytes>,
}

/// State diff from SQD Network.
///
/// Represents a single storage slot or account field change within a block.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StateDiff {
    pub transaction_index: u32,

    #[serde(default)]
    pub address: Option<Address>,

    /// Storage key, or special key: "balance", "code", "nonce".
    #[serde(default)]
    pub key: Option<String>,

    /// Kind of state change: "=" (unchanged), "+" (created), "*" (modified), "-" (deleted).
    #[serde(default)]
    pub kind: Option<String>,

    #[serde(default)]
    pub prev: Option<String>,

    #[serde(default)]
    pub next: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    const FIXTURE: &str = include_str!("../fixtures/polygon_block_65000000.json");

    #[test]
    fn deserialize_polygon_block_fixture() {
        let block: Block = serde_json::from_str(FIXTURE).unwrap();

        assert_eq!(block.header.number, 65_000_000);
        assert_eq!(
            block.header.hash.unwrap(),
            b256!("40a243f82db77b7557e7b56808e93ef72c5bc83f16ad5ede236b496a78736eb6")
        );
        assert_eq!(
            block.header.parent_hash.unwrap(),
            b256!("5fa5e769a0b2bd46a7f857af0507587d2d02f67305d917dfdeeb2d6087b21a18")
        );
        assert_eq!(block.header.timestamp.unwrap(), 1_733_157_748);
        assert_eq!(
            block.header.miner.unwrap(),
            address!("0000000000000000000000000000000000000000")
        );
        assert_eq!(block.transactions.len(), 3);
    }

    #[test]
    fn header_gas_fields_parse_as_hex() {
        let block: Block = serde_json::from_str(FIXTURE).unwrap();

        // gasLimit: "0x1c9c380" = 30_000_000
        assert_eq!(block.header.gas_limit.unwrap().as_u64(), 30_000_000);
        // gasUsed: "0xd3dd42" = 13_884_738
        assert_eq!(block.header.gas_used.unwrap().as_u64(), 13_884_738);
        // baseFeePerGas: "0x2ef60d024d" = 201_696_543_309
        assert_eq!(
            block.header.base_fee_per_gas.unwrap().as_u64(),
            201_696_543_309
        );
        // difficulty: "0x19" = 25
        assert_eq!(block.header.difficulty.unwrap().as_u64(), 25);
    }

    #[test]
    fn transaction_eip1559_fields() {
        let block: Block = serde_json::from_str(FIXTURE).unwrap();
        let tx = &block.transactions[0];

        assert_eq!(tx.transaction_index, 0);
        assert_eq!(
            tx.from.unwrap(),
            address!("706c7fa886ccaf510e570c5fa91f5988b15a8a56")
        );
        assert_eq!(
            tx.to.unwrap(),
            address!("e957a692c97566efc85f995162fa404091232b2e")
        );
        assert_eq!(tx.nonce.unwrap(), 34547);
        assert_eq!(tx.tx_type.unwrap(), 2); // EIP-1559
        assert_eq!(tx.status.unwrap(), 1);
        assert_eq!(tx.chain_id.unwrap(), 137);
        assert_eq!(tx.y_parity.unwrap(), 1);
        assert_eq!(tx.value.unwrap().as_u64(), 0);

        // EIP-1559 fields present
        assert!(tx.max_fee_per_gas.is_some());
        assert!(tx.max_priority_fee_per_gas.is_some());
    }

    #[test]
    fn transaction_legacy_fields() {
        let block: Block = serde_json::from_str(FIXTURE).unwrap();
        let tx = &block.transactions[1]; // type 0 legacy

        assert_eq!(tx.transaction_index, 1);
        assert_eq!(tx.tx_type.unwrap(), 0);
        assert_eq!(tx.status.unwrap(), 1);

        // Legacy: gasPrice present, EIP-1559 fields absent
        assert!(tx.gas_price.is_some());
        assert!(tx.max_fee_per_gas.is_none());
        assert!(tx.max_priority_fee_per_gas.is_none());
        assert!(tx.y_parity.is_none());
    }

    #[test]
    fn transaction_receipt_fields() {
        let block: Block = serde_json::from_str(FIXTURE).unwrap();
        let tx = &block.transactions[0];

        // gasUsed: "0x36f52" = 225_106
        assert_eq!(tx.gas_used.unwrap().as_u64(), 0x36f52);
        // cumulativeGasUsed: "0x36f52" = 225_106
        assert_eq!(tx.cumulative_gas_used.unwrap().as_u64(), 0x36f52);
        assert!(tx.effective_gas_price.is_some());
        // contractAddress is null for non-deployment txs
        assert!(tx.contract_address.is_none());
    }

    #[test]
    fn transaction_contract_deploy_to_is_none() {
        // A contract deployment has `to: null`
        let json = r#"{
            "transactionIndex": 0,
            "hash": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "from": "0x0000000000000000000000000000000000000001",
            "to": null,
            "input": "0x6060",
            "value": "0x0",
            "nonce": 0,
            "gas": "0x100000",
            "gasPrice": "0x1",
            "type": 0,
            "status": 1,
            "gasUsed": "0x50000",
            "cumulativeGasUsed": "0x50000",
            "effectiveGasPrice": "0x1",
            "contractAddress": "0x0000000000000000000000000000000000000099"
        }"#;
        let tx: Transaction = serde_json::from_str(json).unwrap();
        assert!(tx.to.is_none());
        assert_eq!(
            tx.contract_address.unwrap(),
            address!("0000000000000000000000000000000000000099")
        );
    }

    #[test]
    fn block_with_empty_transactions() {
        let json = r#"{"header": {"number": 1}, "transactions": []}"#;
        let block: Block = serde_json::from_str(json).unwrap();
        assert_eq!(block.header.number, 1);
        assert!(block.transactions.is_empty());
    }

    #[test]
    fn block_with_no_transactions_field() {
        let json = r#"{"header": {"number": 1}}"#;
        let block: Block = serde_json::from_str(json).unwrap();
        assert!(block.transactions.is_empty());
    }

    #[test]
    fn minimal_header_only_number() {
        let json = r#"{"header": {"number": 42}}"#;
        let block: Block = serde_json::from_str(json).unwrap();
        assert_eq!(block.header.number, 42);
        assert!(block.header.hash.is_none());
        assert!(block.header.parent_hash.is_none());
        assert!(block.header.timestamp.is_none());
        assert!(block.header.gas_limit.is_none());
    }

    #[test]
    fn state_diff_deserialize() {
        let json = r#"{
            "transactionIndex": 5,
            "address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045",
            "key": "0x0000000000000000000000000000000000000000000000000000000000000001",
            "kind": "*",
            "prev": "0x0000000000000000000000000000000000000000000000000000000000000064",
            "next": "0x00000000000000000000000000000000000000000000000000000000000000c8"
        }"#;
        let diff: StateDiff = serde_json::from_str(json).unwrap();
        assert_eq!(diff.transaction_index, 5);
        assert_eq!(
            diff.address.unwrap(),
            address!("d8da6bf26964af9d7eed9e03e53415d37aa96045")
        );
        assert_eq!(diff.kind.as_deref(), Some("*"));
    }

    #[test]
    fn state_diff_balance_key() {
        let json = r#"{
            "transactionIndex": 0,
            "address": "0x0000000000000000000000000000000000000001",
            "key": "balance",
            "kind": "*",
            "prev": "0x100",
            "next": "0x200"
        }"#;
        let diff: StateDiff = serde_json::from_str(json).unwrap();
        assert_eq!(diff.key.as_deref(), Some("balance"));
    }

    #[test]
    fn hex_u64_from_string() {
        let val: HexU64 = serde_json::from_str(r#""0x1c9c380""#).unwrap();
        assert_eq!(val.as_u64(), 30_000_000);
    }

    #[test]
    fn hex_u64_zero() {
        let val: HexU64 = serde_json::from_str(r#""0x0""#).unwrap();
        assert_eq!(val.as_u64(), 0);
    }

    #[test]
    fn hex_u64_max() {
        let val: HexU64 = serde_json::from_str(r#""0xffffffffffffffff""#).unwrap();
        assert_eq!(val.as_u64(), u64::MAX);
    }

    #[test]
    fn hex_u64_serialize_round_trip() {
        let val = HexU64::from(12345u64);
        let json = serde_json::to_string(&val).unwrap();
        let decoded: HexU64 = serde_json::from_str(&json).unwrap();
        assert_eq!(val, decoded);
    }

    #[test]
    fn hex_u64_from_u256_large_value() {
        // baseFeePerGas "0x2ef60d024d" = 201_696_543_309, fits in u64
        let val: HexU64 = serde_json::from_str(r#""0x2ef60d024d""#).unwrap();
        assert_eq!(val.as_u64(), 0x2ef60d024d);
    }

    #[test]
    fn transaction_input_is_bytes() {
        let block: Block = serde_json::from_str(FIXTURE).unwrap();
        let tx = &block.transactions[2]; // simple ERC-20 transfer
        let input = tx.input.as_ref().unwrap();
        // sighash for transfer(address,uint256) = 0xa9059cbb
        assert_eq!(&input[..4], &[0xa9, 0x05, 0x9c, 0xbb]);
    }
}
