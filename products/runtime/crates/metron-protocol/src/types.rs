//! Validated primitives and structural hashing.
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest as _, Sha256};
use thiserror::Error;

pub const MAX_ENVELOPE: usize = 64 * 1024;
pub const MAX_INTERNAL: usize = 256 * 1024;
pub const MAX_FIXTURE: usize = 1024;
pub const MAX_CONTEXT: usize = 4 * 1024;
pub const MAX_TEXT: usize = 1024;
pub const MAX_SQL_INTEGER: u64 = i64::MAX as u64;

#[derive(Clone, Debug, Eq, PartialEq, Error)]
pub enum ValidationError {
    #[error("invalid identifier")]
    Identifier,
    #[error("invalid digest")]
    Digest,
    #[error("invalid decimal usage")]
    Decimal,
    #[error("value exceeds limit")]
    Limit,
    #[error("unsupported schema version")]
    Version,
    #[error("elapsed-time budgets are unsupported")]
    UnsupportedBudgetDimension,
    #[error("materialized binding mismatch")]
    Binding,
    #[error("invalid JSON: {0}")]
    Json(String),
}

macro_rules! id_type {
    ($($name:ident),+ $(,)?) => {$ (
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
                let value = value.into();
                if value.is_empty() || value.len() > 128 || !value.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')) { return Err(ValidationError::Identifier); }
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str { &self.0 }
        }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
            }
        }
    )+};
}
id_type!(
    TaskId,
    PrincipalId,
    CommandId,
    WorkerId,
    ProducerReceiptId,
    ApprovalId,
    EvidenceRef,
    InboxId
);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct Digest(String);
impl Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ValidationError::Digest);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn bytes(&self) -> [u8; 32] {
        let mut result = [0; 32];
        for (i, chunk) in self.0.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let nibble = |b: u8| if b <= b'9' { b - b'0' } else { b - b'a' + 10 };
            result[i] = nibble(chunk[0]) * 16 + nibble(chunk[1]);
        }
        result
    }
    fn from_bytes(bytes: &[u8]) -> Self {
        const HEX: &[u8] = b"0123456789abcdef";
        let mut result = String::with_capacity(64);
        for b in bytes {
            result.push(HEX[(b >> 4) as usize] as char);
            result.push(HEX[(b & 15) as usize] as char);
        }
        Self(result)
    }
}
impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
pub fn content_digest(bytes: &[u8]) -> Digest {
    Digest::from_bytes(&Sha256::digest(bytes))
}
pub fn structural_digest(domain: &str, fields: &[&[u8]]) -> Result<Digest, ValidationError> {
    if !domain.is_ascii() || domain.len() > u16::MAX as usize || fields.len() > u32::MAX as usize {
        return Err(ValidationError::Limit);
    }
    let mut h = Sha256::new();
    h.update(b"metron-runtime\0");
    h.update((domain.len() as u16).to_be_bytes());
    h.update(domain.as_bytes());
    h.update((fields.len() as u32).to_be_bytes());
    for field in fields {
        h.update((field.len() as u64).to_be_bytes());
        h.update(field);
    }
    Ok(Digest::from_bytes(&h.finalize()))
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct SqlU64(u64);
impl SqlU64 {
    pub fn new(value: u64) -> Result<Self, ValidationError> {
        if value > MAX_SQL_INTEGER {
            Err(ValidationError::Limit)
        } else {
            Ok(Self(value))
        }
    }
    pub fn get(self) -> u64 {
        self.0
    }
    pub fn checked_add(self, value: u64) -> Result<Self, ValidationError> {
        Self::new(self.0.checked_add(value).ok_or(ValidationError::Limit)?)
    }
}
impl<'de> Deserialize<'de> for SqlU64 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(u64::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct DecimalUsage(String);
impl DecimalUsage {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_ENVELOPE
            || !value.bytes().all(|b| b.is_ascii_digit())
            || (value.len() > 1 && value.starts_with('0'))
        {
            return Err(ValidationError::Decimal);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn as_u64(&self) -> Option<u64> {
        self.0.parse().ok()
    }
}
impl<'de> Deserialize<'de> for DecimalUsage {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct BoundedText<const N: usize>(String);
impl<const N: usize> BoundedText<N> {
    pub fn new(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.len() > N {
            Err(ValidationError::Limit)
        } else {
            Ok(Self(value))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de, const N: usize> Deserialize<'de> for BoundedText<N> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SchemaVersion(u16);
impl SchemaVersion {
    pub const V1: Self = Self(1);
}
impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        if u16::deserialize(d)? == 1 {
            Ok(Self::V1)
        } else {
            Err(serde::de::Error::custom(ValidationError::Version))
        }
    }
}

pub fn parse_envelope<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ValidationError> {
    if bytes.len() > MAX_ENVELOPE {
        return Err(ValidationError::Limit);
    }
    serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))
}
/// Parse a bounded internal command or journal record before allocating its values.
pub fn parse_internal<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ValidationError> {
    if bytes.len() > MAX_INTERNAL {
        return Err(ValidationError::Limit);
    }
    serde_json::from_slice(bytes).map_err(|e| ValidationError::Json(e.to_string()))
}
pub fn canonical_bytes<T: Serialize>(value: &T, max: usize) -> Result<Vec<u8>, ValidationError> {
    let bytes = serde_json::to_vec(value).map_err(|e| ValidationError::Json(e.to_string()))?;
    if bytes.len() > max {
        return Err(ValidationError::Limit);
    }
    Ok(bytes)
}
