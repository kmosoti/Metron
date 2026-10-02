//! A small, deterministic random number generator.
//!
//! The kernel must be replayable from a seed on every platform, including
//! `wasm32-wasip1`, so it carries its own generator instead of depending on an
//! OS entropy source. The algorithm is xoshiro256** seeded through splitmix64,
//! both public-domain designs by Blackman and Vigna.

use serde::{Deserialize, Serialize};

/// Deterministic generator (xoshiro256**).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng {
    state: [u64; 4],
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

impl Rng {
    /// Creates a generator from a 64-bit seed.
    #[must_use]
    pub fn seed_from_u64(seed: u64) -> Self {
        let mut s = seed;
        let state = [
            splitmix64(&mut s),
            splitmix64(&mut s),
            splitmix64(&mut s),
            splitmix64(&mut s),
        ];
        Self { state }
    }

    /// Returns the next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.state;
        let result = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        result
    }

    /// Returns a float uniformly distributed in `[0, 1)` with 53 random bits.
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Returns an integer uniformly distributed in `[0, n)`. Panics if `n == 0`.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "Rng::below requires n > 0");
        // Lemire's nearly divisionless method.
        let mut x = self.next_u64();
        let mut m = u128::from(x) * u128::from(n);
        let mut l = m as u64;
        if l < n {
            let t = n.wrapping_neg() % n;
            while l < t {
                x = self.next_u64();
                m = u128::from(x) * u128::from(n);
                l = m as u64;
            }
        }
        (m >> 64) as u64
    }

    /// Returns a `usize` uniformly distributed in `[0, n)`. Panics if `n == 0`.
    pub fn below_usize(&mut self, n: usize) -> usize {
        self.below(n as u64) as usize
    }

    /// Returns `true` with probability `p`.
    pub fn chance(&mut self, p: f64) -> bool {
        self.next_f64() < p
    }

    /// Returns a uniformly random boolean.
    pub fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }

    /// Picks a uniformly random element, or `None` if `items` is empty.
    pub fn choose<'a, T>(&mut self, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            None
        } else {
            Some(&items[self.below_usize(items.len())])
        }
    }

    /// Shuffles `items` in place (Fisher–Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below_usize(i + 1);
            items.swap(i, j);
        }
    }

    /// Derives an independent child generator.
    pub fn fork(&mut self) -> Rng {
        Rng::seed_from_u64(self.next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_seed() {
        let mut a = Rng::seed_from_u64(42);
        let mut b = Rng::seed_from_u64(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = Rng::seed_from_u64(43);
        assert_ne!(a.next_u64(), c.next_u64());
    }

    #[test]
    fn below_stays_in_range_and_covers_values() {
        let mut r = Rng::seed_from_u64(7);
        let mut seen = [false; 5];
        for _ in 0..1000 {
            let v = r.below(5);
            assert!(v < 5);
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
        assert!((0..1000).all(|_| r.next_f64() < 1.0));
    }

    #[test]
    fn below_is_uniform_for_a_non_power_of_two() {
        // Prior: Lemire's nearly divisionless method is unbiased. Chi-square
        // over 1,000 bins with 1,000,000 draws; the 0.1% critical value for
        // 999 degrees of freedom is about 1,144.
        let mut r = Rng::seed_from_u64(99);
        let bins = 1_000u64;
        let draws = 1_000_000u64;
        let mut counts = vec![0u64; bins as usize];
        for _ in 0..draws {
            counts[r.below(bins) as usize] += 1;
        }
        let expected = draws as f64 / bins as f64;
        let chi2: f64 = counts
            .iter()
            .map(|&c| (c as f64 - expected).powi(2) / expected)
            .sum();
        println!("chi-square over {bins} bins: {chi2:.1} (df {})", bins - 1);
        assert!(chi2 < 1_144.0, "chi-square {chi2}");
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = Rng::seed_from_u64(1);
        let mut v: Vec<u32> = (0..20).collect();
        r.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..20).collect::<Vec<_>>());
    }

    #[test]
    fn matches_the_reference_implementation() {
        // Prior: the constants and the seeding procedure are the published
        // ones. Reference vectors were produced by compiling Blackman and
        // Vigna's `xoshiro256starstar.c` and `splitmix64.c` (prng.di.unimi.it)
        // with the state seeded by four successive splitmix64 outputs, which
        // is how `seed_from_u64` seeds.
        let vectors: [(u64, [u64; 6]); 5] = [
            (
                0,
                [
                    0x99ec5f36cb75f2b4,
                    0xbf6e1f784956452a,
                    0x1a5f849d4933e6e0,
                    0x6aa594f1262d2d2c,
                    0xbba5ad4a1f842e59,
                    0xffef8375d9ebcaca,
                ],
            ),
            (
                1,
                [
                    0xb3f2af6d0fc710c5,
                    0x853b559647364cea,
                    0x92f89756082a4514,
                    0x642e1c7bc266a3a7,
                    0xb27a48e29a233673,
                    0x24c123126ffda722,
                ],
            ),
            (
                42,
                [
                    0x15780b2e0c2ec716,
                    0x6104d9866d113a7e,
                    0xae17533239e499a1,
                    0xecb8ad4703b360a1,
                    0xfde6dc7fe2ec5e64,
                    0xc50da53101795238,
                ],
            ),
            (
                0xDEAD_BEEF,
                [
                    0xc5555444a74d7e83,
                    0x65c30d37b4b16e38,
                    0x54f773200a4efa23,
                    0x429aed75fb958af7,
                    0xfb0e1dd69c255b2e,
                    0x9d6d02ec58814a27,
                ],
            ),
            (
                u64::MAX,
                [
                    0x8f5520d52a7ead08,
                    0xc476a018caa1802d,
                    0x81de31c0d260469e,
                    0xbf658d7e065f3c2f,
                    0x913593fda1bca32a,
                    0xbb535e93941ba525,
                ],
            ),
        ];
        for (seed, expected) in vectors {
            let mut r = Rng::seed_from_u64(seed);
            let got: Vec<u64> = (0..6).map(|_| r.next_u64()).collect();
            assert_eq!(got, expected, "seed {seed}");
        }
    }
}
