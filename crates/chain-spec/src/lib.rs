use alloy_primitives::{Address, TxKind, U256};
use evm_state_data_types::{BlockHeader, Transaction};
use revm::context::{BlockEnv, TxEnv};
use revm::primitives::hardfork::SpecId;

/// How a hardfork is activated — by block number or by timestamp.
#[derive(Debug, Clone, Copy)]
pub enum HardforkCondition {
    /// Activates at a specific block number (pre-Merge style).
    Block(u64),
    /// Activates at a specific timestamp (post-Merge style, used by Ethereum mainnet).
    Timestamp(u64),
}

/// A hardfork activation: the spec becomes active when the condition is met.
#[derive(Debug, Clone, Copy)]
pub struct HardforkActivation {
    pub condition: HardforkCondition,
    pub spec_id: SpecId,
}

/// Chain specification defining hardfork schedule and chain identity.
#[derive(Debug, Clone)]
pub struct ChainSpec {
    pub chain_id: u64,
    pub name: &'static str,
    /// Sorted chronologically — block-based activations first, then timestamp-based.
    pub hardforks: Vec<HardforkActivation>,
}

impl ChainSpec {
    /// Return the active [`SpecId`] for a given block number and timestamp.
    ///
    /// Walks the hardfork schedule in reverse to find the latest activation
    /// whose condition is satisfied.
    pub fn spec_at(&self, block_number: u64, timestamp: u64) -> SpecId {
        for hf in self.hardforks.iter().rev() {
            let active = match hf.condition {
                HardforkCondition::Block(n) => block_number >= n,
                HardforkCondition::Timestamp(t) => timestamp >= t,
            };
            if active {
                return hf.spec_id;
            }
        }
        SpecId::FRONTIER
    }
}

/// Look up a built-in [`ChainSpec`] by chain ID.
///
/// Returns `None` for unknown chain IDs.
pub fn from_chain_id(chain_id: u64) -> Option<ChainSpec> {
    match chain_id {
        1 => Some(ethereum_mainnet()),
        137 => Some(polygon_mainnet()),
        _ => None,
    }
}

// ── Polygon PoS (chain ID 137) ──────────────────────────────────────

/// Polygon PoS mainnet chain specification.
///
/// Hardfork block numbers sourced from:
/// <https://github.com/maticnetwork/bor/blob/develop/params/config.go>
///
/// Note: Polygon launched post-Byzantium, so early hardforks activate at block 0.
pub fn polygon_mainnet() -> ChainSpec {
    ChainSpec {
        chain_id: 137,
        name: "polygon-mainnet",
        hardforks: vec![
            // Genesis: Polygon launched with Byzantium+ rules from block 0
            HardforkActivation {
                condition: HardforkCondition::Block(0),
                spec_id: SpecId::PETERSBURG,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(3_395_000),
                spec_id: SpecId::ISTANBUL,
            },
            // MuirGlacier only changes the difficulty bomb — Polygon uses
            // Bor consensus so it has no practical effect, but we match the config.
            HardforkActivation {
                condition: HardforkCondition::Block(3_395_000),
                spec_id: SpecId::MUIR_GLACIER,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(14_750_000),
                spec_id: SpecId::BERLIN,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(23_850_000),
                spec_id: SpecId::LONDON,
            },
            // Polygon uses block numbers (not timestamps) for post-Merge forks.
            // There is no "Merge" on Polygon since it uses Bor/Heimdall consensus.
            HardforkActivation {
                condition: HardforkCondition::Block(50_523_000),
                spec_id: SpecId::SHANGHAI,
            },
            // Cancun / "Napoli"
            HardforkActivation {
                condition: HardforkCondition::Block(54_876_000),
                spec_id: SpecId::CANCUN,
            },
            // Prague / "Bhilai"
            HardforkActivation {
                condition: HardforkCondition::Block(73_440_256),
                spec_id: SpecId::PRAGUE,
            },
        ],
    }
}

// ── Ethereum Mainnet (chain ID 1) ──────────────────────────────────

/// Ethereum mainnet chain specification.
///
/// Pre-Merge hardforks activate by block number.
/// Post-Merge hardforks (Shanghai, Cancun, Prague) activate by timestamp.
///
/// Hardfork block numbers sourced from:
/// <https://github.com/ethereum/go-ethereum/blob/master/params/config.go>
pub fn ethereum_mainnet() -> ChainSpec {
    ChainSpec {
        chain_id: 1,
        name: "ethereum-mainnet",
        hardforks: vec![
            HardforkActivation {
                condition: HardforkCondition::Block(0),
                spec_id: SpecId::FRONTIER,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(200_000),
                spec_id: SpecId::FRONTIER_THAWING,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(1_150_000),
                spec_id: SpecId::HOMESTEAD,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(1_920_000),
                spec_id: SpecId::DAO_FORK,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(2_463_000),
                spec_id: SpecId::TANGERINE,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(2_675_000),
                spec_id: SpecId::SPURIOUS_DRAGON,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(4_370_000),
                spec_id: SpecId::BYZANTIUM,
            },
            // Constantinople + Petersburg both at 7_280_000; Petersburg supersedes.
            HardforkActivation {
                condition: HardforkCondition::Block(7_280_000),
                spec_id: SpecId::PETERSBURG,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(9_069_000),
                spec_id: SpecId::ISTANBUL,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(9_200_000),
                spec_id: SpecId::MUIR_GLACIER,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(12_244_000),
                spec_id: SpecId::BERLIN,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(12_965_000),
                spec_id: SpecId::LONDON,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(13_773_000),
                spec_id: SpecId::ARROW_GLACIER,
            },
            HardforkActivation {
                condition: HardforkCondition::Block(15_050_000),
                spec_id: SpecId::GRAY_GLACIER,
            },
            // The Merge (TTD 58750000000000000000000, block ~15_537_394)
            HardforkActivation {
                condition: HardforkCondition::Block(15_537_394),
                spec_id: SpecId::MERGE,
            },
            // Post-Merge: timestamp-based activations
            HardforkActivation {
                condition: HardforkCondition::Timestamp(1_681_338_455),
                spec_id: SpecId::SHANGHAI,
            },
            HardforkActivation {
                condition: HardforkCondition::Timestamp(1_710_338_135),
                spec_id: SpecId::CANCUN,
            },
            HardforkActivation {
                condition: HardforkCondition::Timestamp(1_746_612_311),
                spec_id: SpecId::PRAGUE,
            },
        ],
    }
}

// ── Environment builders ─────────────────────────────────────────────

/// Build a revm [`BlockEnv`] from a SQD [`BlockHeader`].
pub fn block_env_from_header(header: &BlockHeader) -> BlockEnv {
    BlockEnv {
        number: header.number,
        beneficiary: header.miner.unwrap_or(Address::ZERO),
        timestamp: header.timestamp.unwrap_or(0),
        gas_limit: header
            .gas_limit
            .map(|g| g.as_u64())
            .unwrap_or(30_000_000),
        basefee: header.base_fee_per_gas.map(|b| b.as_u64()).unwrap_or(0),
        difficulty: U256::from(header.difficulty.map(|d| d.as_u64()).unwrap_or(0)),
        prevrandao: header.mix_hash,
        blob_excess_gas_and_price: None,
    }
}

/// Build a revm [`TxEnv`] from a SQD [`Transaction`] and the chain's ID.
pub fn tx_env_from_transaction(tx: &Transaction, chain_id: u64) -> TxEnv {
    let kind = match tx.to {
        Some(addr) => TxKind::Call(addr),
        None => TxKind::Create,
    };

    let gas_price = if tx.tx_type == Some(2) {
        // EIP-1559: gas_price = max_fee_per_gas
        tx.max_fee_per_gas.map(|g| g.as_u64()).unwrap_or(0) as u128
    } else {
        tx.gas_price.map(|g| g.as_u64()).unwrap_or(0) as u128
    };

    let gas_priority_fee = if tx.tx_type == Some(2) {
        tx.max_priority_fee_per_gas.map(|g| g.as_u64() as u128)
    } else {
        None
    };

    TxEnv {
        tx_type: tx.tx_type.unwrap_or(0),
        caller: tx.from.unwrap_or(Address::ZERO),
        gas_limit: tx.gas.map(|g| g.as_u64()).unwrap_or(21_000),
        gas_price,
        kind,
        value: U256::from(tx.value.map(|v| v.as_u64()).unwrap_or(0)),
        data: tx.input.clone().unwrap_or_default(),
        nonce: tx.nonce.unwrap_or(0),
        chain_id: Some(chain_id),
        access_list: Default::default(),
        gas_priority_fee,
        blob_hashes: Vec::new(),
        max_fee_per_blob_gas: 0,
        authorization_list: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256, B256, Bytes};
    use evm_state_data_types::HexU64;

    // ── Polygon PoS ───────────────────────────────────────────────────

    #[test]
    fn polygon_genesis_is_petersburg() {
        let spec = polygon_mainnet();
        assert_eq!(spec.spec_at(0, 0), SpecId::PETERSBURG);
        assert_eq!(spec.spec_at(1, 0), SpecId::PETERSBURG);
        assert_eq!(spec.spec_at(3_394_999, 0), SpecId::PETERSBURG);
    }

    #[test]
    fn polygon_istanbul_at_3395000() {
        let spec = polygon_mainnet();
        // MuirGlacier activates at same block, so it wins (later in list)
        assert_eq!(spec.spec_at(3_395_000, 0), SpecId::MUIR_GLACIER);
    }

    #[test]
    fn polygon_berlin_at_14750000() {
        let spec = polygon_mainnet();
        assert_eq!(spec.spec_at(14_749_999, 0), SpecId::MUIR_GLACIER);
        assert_eq!(spec.spec_at(14_750_000, 0), SpecId::BERLIN);
    }

    #[test]
    fn polygon_london_at_23850000() {
        let spec = polygon_mainnet();
        assert_eq!(spec.spec_at(23_849_999, 0), SpecId::BERLIN);
        assert_eq!(spec.spec_at(23_850_000, 0), SpecId::LONDON);
    }

    #[test]
    fn polygon_shanghai_at_50523000() {
        let spec = polygon_mainnet();
        assert_eq!(spec.spec_at(50_522_999, 0), SpecId::LONDON);
        assert_eq!(spec.spec_at(50_523_000, 0), SpecId::SHANGHAI);
    }

    #[test]
    fn polygon_cancun_at_54876000() {
        let spec = polygon_mainnet();
        assert_eq!(spec.spec_at(54_875_999, 0), SpecId::SHANGHAI);
        assert_eq!(spec.spec_at(54_876_000, 0), SpecId::CANCUN);
    }

    #[test]
    fn polygon_prague_at_73440256() {
        let spec = polygon_mainnet();
        assert_eq!(spec.spec_at(73_440_255, 0), SpecId::CANCUN);
        assert_eq!(spec.spec_at(73_440_256, 0), SpecId::PRAGUE);
    }

    #[test]
    fn polygon_chain_id() {
        assert_eq!(polygon_mainnet().chain_id, 137);
    }

    #[test]
    fn polygon_recent_block_is_cancun_or_later() {
        let spec = polygon_mainnet();
        // Block 65_000_000 (our test fixture) should be Cancun
        assert_eq!(spec.spec_at(65_000_000, 0), SpecId::CANCUN);
    }

    // ── Ethereum Mainnet ──────────────────────────────────────────────

    #[test]
    fn ethereum_chain_id() {
        assert_eq!(ethereum_mainnet().chain_id, 1);
    }

    #[test]
    fn ethereum_genesis_is_frontier() {
        let spec = ethereum_mainnet();
        assert_eq!(spec.spec_at(0, 0), SpecId::FRONTIER);
        assert_eq!(spec.spec_at(199_999, 0), SpecId::FRONTIER);
    }

    #[test]
    fn ethereum_homestead_at_1150000() {
        let spec = ethereum_mainnet();
        assert_eq!(spec.spec_at(1_149_999, 0), SpecId::FRONTIER_THAWING);
        assert_eq!(spec.spec_at(1_150_000, 0), SpecId::HOMESTEAD);
        assert_eq!(spec.spec_at(1_919_999, 0), SpecId::HOMESTEAD);
    }

    #[test]
    fn ethereum_dao_fork_at_1920000() {
        let spec = ethereum_mainnet();
        assert_eq!(spec.spec_at(1_919_999, 0), SpecId::HOMESTEAD);
        assert_eq!(spec.spec_at(1_920_000, 0), SpecId::DAO_FORK);
    }

    #[test]
    fn ethereum_byzantium_at_4370000() {
        let spec = ethereum_mainnet();
        assert_eq!(spec.spec_at(4_369_999, 0), SpecId::SPURIOUS_DRAGON);
        assert_eq!(spec.spec_at(4_370_000, 0), SpecId::BYZANTIUM);
    }

    #[test]
    fn ethereum_berlin_at_12244000() {
        let spec = ethereum_mainnet();
        assert_eq!(spec.spec_at(12_243_999, 0), SpecId::MUIR_GLACIER);
        assert_eq!(spec.spec_at(12_244_000, 0), SpecId::BERLIN);
    }

    #[test]
    fn ethereum_london_at_12965000() {
        let spec = ethereum_mainnet();
        assert_eq!(spec.spec_at(12_964_999, 0), SpecId::BERLIN);
        assert_eq!(spec.spec_at(12_965_000, 0), SpecId::LONDON);
    }

    #[test]
    fn ethereum_merge_at_15537394() {
        let spec = ethereum_mainnet();
        assert_eq!(spec.spec_at(15_537_393, 0), SpecId::GRAY_GLACIER);
        assert_eq!(spec.spec_at(15_537_394, 0), SpecId::MERGE);
    }

    #[test]
    fn ethereum_shanghai_by_timestamp() {
        let spec = ethereum_mainnet();
        // Post-Merge: block number is high enough, timestamp decides
        let block = 17_034_870;
        assert_eq!(spec.spec_at(block, 1_681_338_454), SpecId::MERGE);
        assert_eq!(spec.spec_at(block, 1_681_338_455), SpecId::SHANGHAI);
    }

    #[test]
    fn ethereum_cancun_by_timestamp() {
        let spec = ethereum_mainnet();
        let block = 19_426_587;
        assert_eq!(spec.spec_at(block, 1_710_338_134), SpecId::SHANGHAI);
        assert_eq!(spec.spec_at(block, 1_710_338_135), SpecId::CANCUN);
    }

    #[test]
    fn ethereum_prague_by_timestamp() {
        let spec = ethereum_mainnet();
        let block = 22_431_084;
        assert_eq!(spec.spec_at(block, 1_746_612_310), SpecId::CANCUN);
        assert_eq!(spec.spec_at(block, 1_746_612_311), SpecId::PRAGUE);
    }

    // ── from_chain_id ─────────────────────────────────────────────────

    #[test]
    fn from_chain_id_known() {
        let eth = from_chain_id(1).unwrap();
        assert_eq!(eth.chain_id, 1);
        assert_eq!(eth.name, "ethereum-mainnet");

        let polygon = from_chain_id(137).unwrap();
        assert_eq!(polygon.chain_id, 137);
        assert_eq!(polygon.name, "polygon-mainnet");
    }

    #[test]
    fn from_chain_id_unknown() {
        assert!(from_chain_id(999).is_none());
    }

    // ── block_env_from_header ─────────────────────────────────────────

    #[test]
    fn block_env_from_polygon_header() {
        let header = BlockHeader {
            number: 65_000_000,
            hash: Some(b256!(
                "40a243f82db77b7557e7b56808e93ef72c5bc83f16ad5ede236b496a78736eb6"
            )),
            parent_hash: Some(b256!(
                "5fa5e769a0b2bd46a7f857af0507587d2d02f67305d917dfdeeb2d6087b21a18"
            )),
            timestamp: Some(1_733_157_748),
            nonce: None,
            sha3_uncles: None,
            transactions_root: None,
            state_root: None,
            receipts_root: None,
            mix_hash: Some(B256::ZERO),
            miner: Some(Address::ZERO),
            difficulty: Some(HexU64::from(25)),
            extra_data: None,
            size: None,
            gas_limit: Some(HexU64::from(30_000_000)),
            gas_used: Some(HexU64::from(13_884_738)),
            base_fee_per_gas: Some(HexU64::from(201_696_543_309u64)),
            l1_block_number: None,
        };

        let env = block_env_from_header(&header);

        assert_eq!(env.number, 65_000_000);
        assert_eq!(env.beneficiary, Address::ZERO);
        assert_eq!(env.timestamp, 1_733_157_748);
        assert_eq!(env.gas_limit, 30_000_000);
        assert_eq!(env.basefee, 201_696_543_309);
        assert_eq!(env.difficulty, U256::from(25));
        assert_eq!(env.prevrandao, Some(B256::ZERO));
    }

    #[test]
    fn block_env_defaults_for_missing_fields() {
        let header = BlockHeader {
            number: 1,
            hash: None,
            parent_hash: None,
            timestamp: None,
            nonce: None,
            sha3_uncles: None,
            transactions_root: None,
            state_root: None,
            receipts_root: None,
            mix_hash: None,
            miner: None,
            difficulty: None,
            extra_data: None,
            size: None,
            gas_limit: None,
            gas_used: None,
            base_fee_per_gas: None,
            l1_block_number: None,
        };

        let env = block_env_from_header(&header);

        assert_eq!(env.number, 1);
        assert_eq!(env.beneficiary, Address::ZERO);
        assert_eq!(env.timestamp, 0);
        assert_eq!(env.gas_limit, 30_000_000); // default
        assert_eq!(env.basefee, 0);
    }

    // ── tx_env_from_transaction ───────────────────────────────────────

    #[test]
    fn tx_env_from_eip1559_transaction() {
        let tx = Transaction {
            transaction_index: 0,
            hash: None,
            from: Some(address!("706c7fa886ccaf510e570c5fa91f5988b15a8a56")),
            to: Some(address!("e957a692c97566efc85f995162fa404091232b2e")),
            input: Some(Bytes::from(vec![0xa1, 0x30, 0x5b, 0x17])),
            value: Some(HexU64::from(0)),
            nonce: Some(34547),
            gas: Some(HexU64::from(235_648)),
            gas_price: Some(HexU64::from(66_870_608_961_598u64)),
            max_fee_per_gas: Some(HexU64::from(66_870_608_961_598u64)),
            max_priority_fee_per_gas: Some(HexU64::from(66_870_608_961_598u64)),
            y_parity: Some(1),
            chain_id: Some(137),
            gas_used: None,
            cumulative_gas_used: None,
            effective_gas_price: None,
            contract_address: None,
            tx_type: Some(2),
            status: Some(1),
            sighash: None,
        };

        let env = tx_env_from_transaction(&tx, 137);

        assert_eq!(env.tx_type, 2);
        assert_eq!(
            env.caller,
            address!("706c7fa886ccaf510e570c5fa91f5988b15a8a56")
        );
        assert_eq!(env.gas_limit, 235_648);
        // EIP-1559: gas_price = max_fee_per_gas
        assert_eq!(env.gas_price, 66_870_608_961_598u128);
        assert_eq!(
            env.gas_priority_fee,
            Some(66_870_608_961_598u128)
        );
        assert_eq!(env.nonce, 34547);
        assert_eq!(env.chain_id, Some(137));
        assert_eq!(env.value, U256::ZERO);
        assert_eq!(env.data.as_ref(), &[0xa1, 0x30, 0x5b, 0x17]);

        match env.kind {
            TxKind::Call(addr) => {
                assert_eq!(addr, address!("e957a692c97566efc85f995162fa404091232b2e"));
            }
            _ => panic!("expected Call"),
        }
    }

    #[test]
    fn tx_env_from_legacy_transaction() {
        let tx = Transaction {
            transaction_index: 1,
            hash: None,
            from: Some(address!("13436214c23100c2e9cc499b957f8cdaaee4b1d8")),
            to: Some(address!("f185b0cdfd3dec4d46fd7131b739de0b8d252eea")),
            input: Some(Bytes::from(vec![0x19, 0x0e, 0x8f, 0xbf])),
            value: Some(HexU64::from(0)),
            nonce: Some(14919),
            gas: Some(HexU64::from(400_000)),
            gas_price: Some(HexU64::from(925_000_000_000u64)),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            y_parity: None,
            chain_id: Some(137),
            gas_used: None,
            cumulative_gas_used: None,
            effective_gas_price: None,
            contract_address: None,
            tx_type: Some(0),
            status: Some(1),
            sighash: None,
        };

        let env = tx_env_from_transaction(&tx, 137);

        assert_eq!(env.tx_type, 0);
        assert_eq!(env.gas_price, 925_000_000_000u128);
        assert!(env.gas_priority_fee.is_none());
        assert_eq!(env.nonce, 14919);
    }

    #[test]
    fn tx_env_contract_creation() {
        let tx = Transaction {
            transaction_index: 0,
            hash: None,
            from: Some(address!("0000000000000000000000000000000000000001")),
            to: None, // contract creation
            input: Some(Bytes::from(vec![0x60, 0x60])),
            value: Some(HexU64::from(0)),
            nonce: Some(0),
            gas: Some(HexU64::from(1_000_000)),
            gas_price: Some(HexU64::from(1)),
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            y_parity: None,
            chain_id: Some(137),
            gas_used: None,
            cumulative_gas_used: None,
            effective_gas_price: None,
            contract_address: None,
            tx_type: Some(0),
            status: None,
            sighash: None,
        };

        let env = tx_env_from_transaction(&tx, 137);

        match env.kind {
            TxKind::Create => {}
            _ => panic!("expected Create"),
        }
    }
}
