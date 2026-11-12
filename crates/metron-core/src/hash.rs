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
    /// Map entries are sorted by their JSON keys recursively. Struct fields
    /// retain their serialization order, including when `serde_json/preserve_order`
    /// is enabled by another workspace dependency.
    pub fn of_json<T: Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        let bytes = crate::canonical::to_vec(value)?;
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
    use crate::canonical;

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
    #[test]
    fn nested_maps_are_sorted_but_struct_fields_keep_declaration_order() {
        #[derive(Serialize)]
        struct Record {
            zebra: u8,
            alpha: serde_json::Value,
        }
        let value: serde_json::Value =
            serde_json::from_str(r#"{"z":1,"a":{"z":2,"a":3}}"#).unwrap();
        let expected = br#"{"a":{"a":3,"z":2},"z":1}"#;
        assert_eq!(canonical::to_vec(&value).unwrap(), expected);
        assert_eq!(
            ContentHash::of_json(&value).unwrap(),
            ContentHash::of_bytes(expected)
        );
        let record = Record {
            zebra: 7,
            alpha: value,
        };
        let expected = br#"{"zebra":7,"alpha":{"a":{"a":3,"z":2},"z":1}}"#;
        assert_eq!(canonical::to_vec(&record).unwrap(), expected);
        assert_eq!(
            ContentHash::of_json(&record).unwrap(),
            ContentHash::of_bytes(expected)
        );
    }

    #[test]
    fn sequence_enum_scalars_and_large_numbers_match_json() {
        #[derive(Serialize)]
        enum Example {
            Unit,
            New(u128),
            Pair(u8, &'static str),
            Record { z: i32, a: bool },
        }
        let examples = [
            Example::Unit,
            Example::New(u128::MAX),
            Example::Pair(1, "x"),
            Example::Record { z: -2, a: true },
        ];
        let expected = br#"["Unit",{"New":340282366920938463463374607431768211455},{"Pair":[1,"x"]},{"Record":{"z":-2,"a":true}}]"#;
        assert_eq!(canonical::to_vec(&examples).unwrap(), expected);
        assert_eq!(
            ContentHash::of_json(&examples).unwrap(),
            ContentHash::of_bytes(expected)
        );
        for value in [0.0, -0.0, 1.25, f64::INFINITY, f64::NAN] {
            assert_eq!(
                canonical::to_vec(&value).unwrap(),
                serde_json::to_vec(&value).unwrap()
            );
        }
        assert_eq!(
            canonical::to_vec(&i128::MIN).unwrap(),
            serde_json::to_vec(&i128::MIN).unwrap()
        );
    }

    #[test]
    fn map_sorting_uses_decoded_keys_and_rejects_invalid_keys() {
        use std::collections::BTreeMap;
        let value = BTreeMap::from([("a", 1), ("\n", 2)]);
        assert_eq!(canonical::to_vec(&value).unwrap(), br#"{"\n":2,"a":1}"#);
        let invalid = BTreeMap::from([((1, 2), true)]);
        let expected = serde_json::to_vec(&invalid).unwrap_err();
        let actual = ContentHash::of_json(&invalid).unwrap_err();
        assert_eq!(actual.to_string(), expected.to_string());
    }
}
