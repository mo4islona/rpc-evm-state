use alloy_primitives::{B256, U256};
use serde::{Deserialize, Serialize};

/// EVM account information stored in the state database.
///
/// Fixed-size layout: nonce (8 bytes) + balance (32 bytes) + code_hash (32 bytes) = 72 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountInfo {
    pub nonce: u64,
    pub balance: U256,
    pub code_hash: B256,
}

impl AccountInfo {
    pub const ENCODED_SIZE: usize = 8 + 32 + 32; // 72 bytes

    /// Encode to a fixed-size 72-byte representation for DB storage.
    pub fn to_bytes(&self) -> [u8; Self::ENCODED_SIZE] {
        let mut buf = [0u8; Self::ENCODED_SIZE];
        buf[0..8].copy_from_slice(&self.nonce.to_be_bytes());
        buf[8..40].copy_from_slice(&self.balance.to_be_bytes::<32>());
        buf[40..72].copy_from_slice(self.code_hash.as_slice());
        buf
    }

    /// Decode from a 72-byte DB value.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AccountDecodeError> {
        if bytes.len() != Self::ENCODED_SIZE {
            return Err(AccountDecodeError::InvalidLength {
                expected: Self::ENCODED_SIZE,
                got: bytes.len(),
            });
        }

        let nonce = u64::from_be_bytes(bytes[0..8].try_into().unwrap());
        let balance = U256::from_be_bytes::<32>(bytes[8..40].try_into().unwrap());
        let code_hash = B256::from_slice(&bytes[40..72]);

        Ok(Self {
            nonce,
            balance,
            code_hash,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AccountDecodeError {
    #[error("invalid account byte length: expected {expected}, got {got}")]
    InvalidLength { expected: usize, got: usize },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    #[test]
    fn round_trip_basic() {
        let info = AccountInfo {
            nonce: 42,
            balance: U256::from(1_000_000u64),
            code_hash: B256::ZERO,
        };
        let bytes = info.to_bytes();
        let decoded = AccountInfo::from_bytes(&bytes).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn round_trip_max_values() {
        let info = AccountInfo {
            nonce: u64::MAX,
            balance: U256::MAX,
            code_hash: b256!("ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
        };
        let bytes = info.to_bytes();
        let decoded = AccountInfo::from_bytes(&bytes).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn round_trip_zero() {
        let info = AccountInfo {
            nonce: 0,
            balance: U256::ZERO,
            code_hash: B256::ZERO,
        };
        let bytes = info.to_bytes();
        let decoded = AccountInfo::from_bytes(&bytes).unwrap();
        assert_eq!(info, decoded);
    }

    #[test]
    fn encoded_size_is_72() {
        let info = AccountInfo {
            nonce: 1,
            balance: U256::from(1u64),
            code_hash: B256::ZERO,
        };
        assert_eq!(info.to_bytes().len(), 72);
    }

    #[test]
    fn decode_wrong_length_fails() {
        let err = AccountInfo::from_bytes(&[0u8; 10]).unwrap_err();
        assert!(err.to_string().contains("expected 72, got 10"));
    }

    #[test]
    fn nonce_is_big_endian() {
        let info = AccountInfo {
            nonce: 1,
            balance: U256::ZERO,
            code_hash: B256::ZERO,
        };
        let bytes = info.to_bytes();
        // nonce=1 in big-endian: 7 zero bytes followed by 0x01
        assert_eq!(bytes[0..7], [0u8; 7]);
        assert_eq!(bytes[7], 1);
    }

    #[test]
    fn json_round_trip() {
        let info = AccountInfo {
            nonce: 42,
            balance: U256::from(1_000_000u64),
            code_hash: B256::ZERO,
        };
        let json = serde_json::to_string(&info).unwrap();
        let decoded: AccountInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(info, decoded);
    }
}
