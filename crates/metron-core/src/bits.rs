//! Fixed-width bit vectors.
//!
//! [`BitVector`] is the shared binary representation used by truth tables in
//! the Boolean laboratory, by binary spatter-code hypervectors in memory, and
//! by version-space masks carried inside capsules. It stores bits in `u64`
//! words, least-significant bit first, and keeps unused tail bits zero so that
//! equality, hashing and Hamming distance are well defined.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Errors from constructing or combining bit vectors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BitError {
    /// Two vectors of different dimension were combined.
    #[error("dimension mismatch: {left} vs {right}")]
    DimensionMismatch {
        /// Dimension of the left operand.
        left: usize,
        /// Dimension of the right operand.
        right: usize,
    },
    /// The number of words does not match the dimension.
    #[error("expected {expected} words for dimension {dim}, got {actual}")]
    WordCount {
        /// Dimension requested.
        dim: usize,
        /// Words required for that dimension.
        expected: usize,
        /// Words supplied.
        actual: usize,
    },
    /// A bit index was out of range.
    #[error("bit index {index} out of range for dimension {dim}")]
    OutOfRange {
        /// The offending index.
        index: usize,
        /// The vector's dimension.
        dim: usize,
    },
}

/// A fixed-width vector of bits.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BitVector {
    dim: usize,
    words: Vec<u64>,
}

impl BitVector {
    /// Number of `u64` words needed for `dim` bits.
    #[must_use]
    pub const fn words_for(dim: usize) -> usize {
        dim.div_ceil(64)
    }

    /// The all-zero vector of the given dimension.
    #[must_use]
    pub fn zeros(dim: usize) -> Self {
        Self {
            dim,
            words: vec![0; Self::words_for(dim)],
        }
    }

    /// The all-one vector of the given dimension.
    #[must_use]
    pub fn ones(dim: usize) -> Self {
        let mut v = Self {
            dim,
            words: vec![u64::MAX; Self::words_for(dim)],
        };
        v.mask_tail();
        v
    }

    /// Builds a vector from raw words. Tail bits beyond `dim` are cleared.
    pub fn from_words(dim: usize, words: Vec<u64>) -> Result<Self, BitError> {
        let expected = Self::words_for(dim);
        if words.len() != expected {
            return Err(BitError::WordCount {
                dim,
                expected,
                actual: words.len(),
            });
        }
        let mut v = Self { dim, words };
        v.mask_tail();
        Ok(v)
    }

    /// Builds a vector by evaluating `f` at every index.
    pub fn from_fn(dim: usize, mut f: impl FnMut(usize) -> bool) -> Self {
        let mut v = Self::zeros(dim);
        for i in 0..dim {
            if f(i) {
                v.words[i / 64] |= 1u64 << (i % 64);
            }
        }
        v
    }

    /// Builds a vector from a slice of booleans.
    #[must_use]
    pub fn from_bools(bits: &[bool]) -> Self {
        Self::from_fn(bits.len(), |i| bits[i])
    }

    /// The number of bits.
    #[must_use]
    pub const fn dim(&self) -> usize {
        self.dim
    }

    /// The underlying words, least-significant bit first.
    #[must_use]
    pub fn words(&self) -> &[u64] {
        &self.words
    }

    /// Returns bit `index`, or `None` if out of range.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<bool> {
        if index >= self.dim {
            return None;
        }
        Some((self.words[index / 64] >> (index % 64)) & 1 == 1)
    }

    /// Returns bit `index`, panicking if out of range.
    #[must_use]
    pub fn bit(&self, index: usize) -> bool {
        self.get(index)
            .unwrap_or_else(|| panic!("bit index {index} out of range for dimension {}", self.dim))
    }

    /// Sets bit `index` to `value`.
    pub fn set(&mut self, index: usize, value: bool) -> Result<(), BitError> {
        if index >= self.dim {
            return Err(BitError::OutOfRange {
                index,
                dim: self.dim,
            });
        }
        let mask = 1u64 << (index % 64);
        if value {
            self.words[index / 64] |= mask;
        } else {
            self.words[index / 64] &= !mask;
        }
        Ok(())
    }

    /// Number of one bits.
    #[must_use]
    pub fn count_ones(&self) -> usize {
        self.words.iter().map(|w| w.count_ones() as usize).sum()
    }

    /// Hamming distance to `other`, or an error on dimension mismatch.
    pub fn try_hamming(&self, other: &Self) -> Result<usize, BitError> {
        if self.dim != other.dim {
            return Err(BitError::DimensionMismatch {
                left: self.dim,
                right: other.dim,
            });
        }
        Ok(self
            .words
            .iter()
            .zip(&other.words)
            .map(|(a, b)| (a ^ b).count_ones() as usize)
            .sum())
    }

    /// Hamming distance to `other`. Panics on dimension mismatch.
    #[must_use]
    pub fn hamming(&self, other: &Self) -> usize {
        self.try_hamming(other)
            .expect("bit vectors must share a dimension")
    }

    /// Cosine-like similarity in `[-1, 1]`: `1 - 2 * hamming / dim`.
    #[must_use]
    pub fn similarity(&self, other: &Self) -> f64 {
        if self.dim == 0 {
            return 1.0;
        }
        1.0 - 2.0 * (self.hamming(other) as f64) / (self.dim as f64)
    }

    /// Bitwise XOR, or an error on dimension mismatch.
    pub fn try_xor(&self, other: &Self) -> Result<Self, BitError> {
        if self.dim != other.dim {
            return Err(BitError::DimensionMismatch {
                left: self.dim,
                right: other.dim,
            });
        }
        Ok(Self {
            dim: self.dim,
            words: self
                .words
                .iter()
                .zip(&other.words)
                .map(|(a, b)| a ^ b)
                .collect(),
        })
    }

    /// Bitwise XOR. Panics on dimension mismatch.
    #[must_use]
    pub fn xor(&self, other: &Self) -> Self {
        self.try_xor(other)
            .expect("bit vectors must share a dimension")
    }

    /// Bitwise AND. Panics on dimension mismatch.
    #[must_use]
    pub fn and(&self, other: &Self) -> Self {
        assert_eq!(self.dim, other.dim, "bit vectors must share a dimension");
        Self {
            dim: self.dim,
            words: self
                .words
                .iter()
                .zip(&other.words)
                .map(|(a, b)| a & b)
                .collect(),
        }
    }

    /// Bitwise NOT within the dimension.
    #[must_use]
    pub fn not(&self) -> Self {
        let mut v = Self {
            dim: self.dim,
            words: self.words.iter().map(|w| !w).collect(),
        };
        v.mask_tail();
        v
    }

    /// Cyclic rotation by `k` positions towards higher indices.
    #[must_use]
    pub fn rotate(&self, k: usize) -> Self {
        if self.dim == 0 {
            return self.clone();
        }
        let k = k % self.dim;
        Self::from_fn(self.dim, |i| self.bit((i + self.dim - k) % self.dim))
    }

    /// Iterates over all bits from index 0 upwards.
    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.dim).map(move |i| self.bit(i))
    }

    /// Indices of all one bits.
    pub fn ones_iter(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.dim).filter(move |&i| self.bit(i))
    }

    /// Lowercase hexadecimal rendering, most-significant word first.
    #[must_use]
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(self.words.len() * 16);
        for w in self.words.iter().rev() {
            s.push_str(&format!("{w:016x}"));
        }
        s
    }

    fn mask_tail(&mut self) {
        let rem = self.dim % 64;
        if rem != 0
            && let Some(last) = self.words.last_mut()
        {
            *last &= (1u64 << rem) - 1;
        }
        if self.dim == 0 {
            self.words.clear();
        }
    }
}

impl fmt::Debug for BitVector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BitVector<{}>({})", self.dim, self.to_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_bits_are_masked() {
        let v = BitVector::ones(70);
        assert_eq!(v.count_ones(), 70);
        assert_eq!(v.words()[1], 0b11_1111);
        let w = BitVector::from_words(5, vec![u64::MAX]).unwrap();
        assert_eq!(w.count_ones(), 5);
    }

    #[test]
    fn hamming_and_similarity() {
        let a = BitVector::from_fn(8, |i| i % 2 == 0);
        let b = BitVector::not(&a);
        assert_eq!(a.hamming(&b), 8);
        assert_eq!(a.similarity(&b), -1.0);
        assert_eq!(a.similarity(&a), 1.0);
        assert!(a.try_hamming(&BitVector::zeros(9)).is_err());
    }

    #[test]
    fn rotation_is_cyclic() {
        let a = BitVector::from_bools(&[true, false, false, false, false]);
        let r = a.rotate(2);
        assert!(r.bit(2));
        assert_eq!(r.count_ones(), 1);
        assert_eq!(a.rotate(5), a);
    }

    #[test]
    fn serde_round_trip() {
        let a = BitVector::from_fn(130, |i| i % 3 == 0);
        let json = serde_json::to_string(&a).unwrap();
        let back: BitVector = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }
}
