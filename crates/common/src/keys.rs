use alloy_primitives::{Address, B256};

/// Database key for the `accounts` table: `[address:20 bytes]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccountKey(pub Address);

impl AccountKey {
    pub const SIZE: usize = 20;

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        *self.0.as_ref()
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KeyDecodeError> {
        if bytes.len() != Self::SIZE {
            return Err(KeyDecodeError::InvalidLength {
                key_type: "AccountKey",
                expected: Self::SIZE,
                got: bytes.len(),
            });
        }
        Ok(Self(Address::from_slice(bytes)))
    }
}

/// Database key for the `storage` table: `[address:20 bytes][slot:32 bytes]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StorageKey {
    pub address: Address,
    pub slot: B256,
}

impl StorageKey {
    pub const SIZE: usize = 20 + 32; // 52 bytes

    pub fn new(address: Address, slot: B256) -> Self {
        Self { address, slot }
    }

    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        let mut buf = [0u8; Self::SIZE];
        buf[0..20].copy_from_slice(self.address.as_slice());
        buf[20..52].copy_from_slice(self.slot.as_slice());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, KeyDecodeError> {
        if bytes.len() != Self::SIZE {
            return Err(KeyDecodeError::InvalidLength {
                key_type: "StorageKey",
                expected: Self::SIZE,
                got: bytes.len(),
            });
        }
        let address = Address::from_slice(&bytes[0..20]);
        let slot = B256::from_slice(&bytes[20..52]);
        Ok(Self { address, slot })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KeyDecodeError {
    #[error("invalid {key_type} byte length: expected {expected}, got {got}")]
    InvalidLength {
        key_type: &'static str,
        expected: usize,
        got: usize,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    #[test]
    fn account_key_round_trip() {
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let key = AccountKey(addr);
        let bytes = key.to_bytes();
        let decoded = AccountKey::from_bytes(&bytes).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn account_key_size_is_20() {
        let key = AccountKey(Address::ZERO);
        assert_eq!(key.to_bytes().len(), 20);
    }

    #[test]
    fn account_key_wrong_length() {
        let err = AccountKey::from_bytes(&[0u8; 10]).unwrap_err();
        assert!(err.to_string().contains("expected 20, got 10"));
    }

    #[test]
    fn storage_key_round_trip() {
        let addr = address!("d8dA6BF26964aF9D7eEd9e03E53415D37aA96045");
        let slot = b256!("0000000000000000000000000000000000000000000000000000000000000005");
        let key = StorageKey::new(addr, slot);
        let bytes = key.to_bytes();
        let decoded = StorageKey::from_bytes(&bytes).unwrap();
        assert_eq!(key, decoded);
    }

    #[test]
    fn storage_key_size_is_52() {
        let key = StorageKey::new(Address::ZERO, B256::ZERO);
        assert_eq!(key.to_bytes().len(), 52);
    }

    #[test]
    fn storage_key_wrong_length() {
        let err = StorageKey::from_bytes(&[0u8; 30]).unwrap_err();
        assert!(err.to_string().contains("expected 52, got 30"));
    }

    #[test]
    fn storage_key_preserves_address_prefix() {
        let addr = address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        let slot = b256!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let key = StorageKey::new(addr, slot);
        let bytes = key.to_bytes();

        // First 20 bytes are the address
        assert_eq!(&bytes[0..20], addr.as_slice());
        // Next 32 bytes are the slot
        assert_eq!(&bytes[20..52], slot.as_slice());
    }

    #[test]
    fn storage_key_ordering_by_address_then_slot() {
        let key_a = StorageKey::new(
            address!("0000000000000000000000000000000000000001"),
            b256!("0000000000000000000000000000000000000000000000000000000000000002"),
        );
        let key_b = StorageKey::new(
            address!("0000000000000000000000000000000000000001"),
            b256!("0000000000000000000000000000000000000000000000000000000000000003"),
        );
        let key_c = StorageKey::new(
            address!("0000000000000000000000000000000000000002"),
            b256!("0000000000000000000000000000000000000000000000000000000000000001"),
        );

        let bytes_a = key_a.to_bytes();
        let bytes_b = key_b.to_bytes();
        let bytes_c = key_c.to_bytes();

        // Lexicographic ordering: a < b (same address, slot 2 < 3)
        assert!(bytes_a < bytes_b);
        // Lexicographic ordering: b < c (address 1 < address 2)
        assert!(bytes_b < bytes_c);
    }
}
