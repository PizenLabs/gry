//! MinHash fingerprints for near-clone detection.
//!
//! 64 bottom-k hashes over token shingles. The fingerprint is deterministic for
//! the same token stream, independent of hash-map iteration order.

use crate::tokenize::{split_identifier, split_text, stable_hash};

pub const K: usize = 64;
pub const SHINGLE: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub hashes: [u64; K],
}

impl Fingerprint {
    /// Bottom-k MinHash over token shingles.
    ///
    /// This keeps the k smallest shingle hashes instead of maintaining k
    /// independent permutation minima. The estimate has the same Jaccard
    /// semantics and avoids the k-fold work (and temporary string joins) of
    /// the classic implementation.
    pub fn from_tokens(tokens: &[String]) -> Self {
        let mut hashes = [u64::MAX; K];
        if tokens.is_empty() {
            return Self { hashes };
        }
        let mut shingle_hashes: Vec<u64> = Vec::with_capacity(tokens.len());
        if tokens.len() < SHINGLE {
            for token in tokens {
                shingle_hashes.push(stable_hash(token));
            }
        } else {
            for window in tokens.windows(SHINGLE) {
                let mut h = 0xcbf2_9ce4_8422_2325u64;
                for token in window {
                    h = h
                        .wrapping_mul(0x0000_0100_0000_01b3)
                        .wrapping_add(stable_hash(token));
                }
                shingle_hashes.push(splitmix64(h));
            }
        }
        let mut len = 0usize;
        for h in shingle_hashes {
            if len == K && h >= hashes[K - 1] {
                continue;
            }
            let pos = match hashes[..len].binary_search(&h) {
                Ok(_) => continue,
                Err(pos) => pos,
            };
            if len < K {
                len += 1;
            }
            if pos < len - 1 {
                hashes.copy_within(pos..len - 1, pos + 1);
            }
            hashes[pos] = h;
        }
        Self { hashes }
    }

    pub fn from_source(source: &str) -> Self {
        let tokens = split_text(source);
        Self::from_tokens(&tokens)
    }

    pub fn from_identifier(name: &str) -> Self {
        let tokens = split_identifier(name);
        Self::from_tokens(&tokens)
    }

    pub fn to_hex(&self) -> String {
        let mut out = String::with_capacity(K * 8);
        for h in self.hashes {
            out.push_str(&format!("{h:016x}"));
        }
        out
    }

    pub fn from_hex(hex: &str) -> Option<Self> {
        if hex.len() != K * 16 {
            return None;
        }
        let mut hashes = [0u64; K];
        for (i, slot) in hashes.iter_mut().enumerate() {
            *slot = u64::from_str_radix(&hex[i * 16..(i + 1) * 16], 16).ok()?;
        }
        Some(Self { hashes })
    }

    /// Estimated Jaccard similarity.
    pub fn jaccard(&self, other: &Self) -> f64 {
        let equal = self
            .hashes
            .iter()
            .zip(other.hashes.iter())
            .filter(|(a, b)| a == b)
            .count();
        equal as f64 / K as f64
    }
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = x;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_fingerprints_match() {
        let a = Fingerprint::from_source("fn handler(req) { parse(req) }");
        let b = Fingerprint::from_source("fn handler(req) { parse(req) }");
        assert_eq!(a.jaccard(&b), 1.0);
    }

    #[test]
    fn similar_sources_score_higher_than_unrelated() {
        let a = Fingerprint::from_source("fn handler(req) { let x = parse(req); save(x); }");
        let b = Fingerprint::from_source("fn handler(req) { let y = parse(req); save(y); }");
        let c = Fingerprint::from_source("fn render(image) { draw_pixels(image) }");
        assert!(a.jaccard(&b) > a.jaccard(&c));
    }

    #[test]
    fn hex_round_trip() {
        let fp = Fingerprint::from_identifier("get_user_by_id");
        let decoded = Fingerprint::from_hex(&fp.to_hex()).unwrap();
        assert_eq!(fp, decoded);
    }
}
