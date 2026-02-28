use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A u64 that serializes/deserializes as a `"0x..."` hex string,
/// matching the SQD Network API convention for numeric fields like
/// `gasLimit`, `gasUsed`, `value`, `gasPrice`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HexU64(u64);

impl HexU64 {
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for HexU64 {
    fn from(v: u64) -> Self {
        Self(v)
    }
}

impl From<HexU64> for u64 {
    fn from(v: HexU64) -> Self {
        v.0
    }
}

impl Serialize for HexU64 {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!("0x{:x}", self.0))
    }
}

impl<'de> Deserialize<'de> for HexU64 {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        let s = s.strip_prefix("0x").unwrap_or(&s);
        u64::from_str_radix(s, 16)
            .map(HexU64)
            .map_err(serde::de::Error::custom)
    }
}
