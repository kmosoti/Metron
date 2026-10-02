//! Content hashing for provenance and the ledger chain.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};
use std::fmt;

/// A SHA-256 digest.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// The all-zero hash, used as the genesis link of a ledger chain.
    pub const GENESIS: ContentHash = ContentHash([0u8; 32]);

    /// Hashes raw bytes.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// Hashes the canonical JSON serialisation of `value`.
    ///
    /// `serde_json` serialises maps in key order and structs in field order,
    /// so the result is deterministic for a given type and value.
    pub fn of_json<T: Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        let bytes = serde_json::to_vec(value)?;
        Ok(Self::of_bytes(&bytes))
    }

    /// Hashes `prev || bytes`, the step of a hash chain.
    #[must_use]
    pub fn chain(prev: &ContentHash, bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(prev.0);
        hasher.update(bytes);
        Self(hasher.finalize().into())
    }

    /// Lowercase hexadecimal rendering.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for b in self.0 {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Parses a 64-character hexadecimal string.
    pub fn from_hex(hex: &str) -> Result<Self, String> {
        if hex.len() != 64 {
            return Err(format!("expected 64 hex characters, got {}", hex.len()));
        }
        let mut out = [0u8; 32];
        for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
            let s = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
            out[i] = u8::from_str_radix(s, 16).map_err(|e| e.to_string())?;
        }
        Ok(Self(out))
    }

    /// A short prefix for display.
    #[must_use]
    pub fn short(&self) -> String {
        self.to_hex()[..12].to_owned()
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({})", self.short())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl Serialize for ContentHash {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        ContentHash::from_hex(&s).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        // SHA-256("abc")
        assert_eq!(
            ContentHash::of_bytes(b"abc").to_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn hex_round_trip_and_serde() {
        let h = ContentHash::of_bytes(b"metron");
        assert_eq!(ContentHash::from_hex(&h.to_hex()).unwrap(), h);
        let json = serde_json::to_string(&h).unwrap();
        let back: ContentHash = serde_json::from_str(&json).unwrap();
        assert_eq!(h, back);
    }

    #[test]
    fn json_hash_is_deterministic() {
        let a = ContentHash::of_json(&serde_json::json!({"b": 1, "a": [1, 2]})).unwrap();
        let b = ContentHash::of_json(&serde_json::json!({"a": [1, 2], "b": 1})).unwrap();
        assert_eq!(a, b);
    }
}
