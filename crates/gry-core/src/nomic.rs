//! Bundled Nomic `nomic-embed-code` token vectors (Apache-2.0).
//!
//! 40,856 tokens × 768 int8 dimensions, unit-normalized and stored ×127.
//! See `third_party/nomic/NOTICE`. gry uses these static token vectors rather
//! than shipping the 7B model: no network, no inference runtime, deterministic.

use once_cell::sync::Lazy;
use std::collections::HashMap;

pub const DIM: usize = 768;
pub const SCALE: f32 = 127.0;

const TOKENS_TEXT: &str = include_str!("../../../third_party/nomic/code_tokens.txt");
const VECTOR_BYTES: &[u8] = include_bytes!("../../../third_party/nomic/code_vectors.bin");

struct Table {
    index: HashMap<&'static str, u32>,
    count: usize,
}

static TABLE: Lazy<Table> = Lazy::new(|| {
    let tokens: Vec<&'static str> = TOKENS_TEXT.lines().collect();
    let mut index = HashMap::with_capacity(tokens.len());
    for (i, token) in tokens.iter().enumerate() {
        index.entry(*token).or_insert(i as u32);
    }
    let header_count = u32::from_le_bytes(VECTOR_BYTES[0..4].try_into().unwrap()) as usize;
    let header_dim = u32::from_le_bytes(VECTOR_BYTES[4..8].try_into().unwrap()) as usize;
    assert_eq!(header_dim, DIM, "unexpected Nomic vector dimension");
    assert_eq!(
        header_count,
        tokens.len(),
        "Nomic token/vector count mismatch"
    );
    assert!(VECTOR_BYTES.len() >= 8 + header_count * DIM);
    Table {
        index,
        count: header_count,
    }
});

/// Total bundled token vocabulary size.
pub fn token_count() -> usize {
    TABLE.count
}

/// Look up the raw int8 vector for a token (zero-copy).
pub fn token_vector(token: &str) -> Option<&'static [i8]> {
    let idx = *TABLE.index.get(token)? as usize;
    let start = 8 + idx * DIM;
    let slice = &VECTOR_BYTES[start..start + DIM];
    // SAFETY: i8 and u8 have identical layout; the blob is a byte array.
    Some(unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const i8, DIM) })
}

/// Whether a token has a pretrained vector.
pub fn has_token(token: &str) -> bool {
    TABLE.index.contains_key(token)
}

/// Average the known token vectors, normalize, and quantize to int8.
pub fn embed_tokens<'a>(tokens: impl IntoIterator<Item = &'a str>) -> Option<Vec<i8>> {
    let mut acc = vec![0.0f32; DIM];
    let mut used = 0usize;
    for token in tokens {
        if let Some(vec) = token_vector(token) {
            for (a, v) in acc.iter_mut().zip(vec.iter()) {
                *a += *v as f32;
            }
            used += 1;
        }
    }
    if used == 0 {
        return None;
    }
    normalize_and_quantize(&mut acc)
}

/// Build a query vector from free text / identifiers.
pub fn embed_text(text: &str) -> Option<Vec<i8>> {
    let tokens = crate::tokenize::split_text(text);
    embed_tokens(tokens.iter().map(String::as_str))
}

fn normalize_and_quantize(values: &mut [f32]) -> Option<Vec<i8>> {
    let norm = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm <= f32::EPSILON {
        return None;
    }
    Some(
        values
            .iter()
            .map(|v| ((v / norm) * SCALE).round().clamp(-127.0, 127.0) as i8)
            .collect(),
    )
}

/// Cosine similarity between two quantized unit vectors.
pub fn cosine_i8(a: &[i8], b: &[i8]) -> f32 {
    let n = a.len().min(b.len()).min(DIM);
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        let x = a[i] as f32;
        let y = b[i] as f32;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Cosine of a float query vector against an int8 stored vector.
pub fn cosine_query_f32(query: &[f32], stored: &[i8]) -> f32 {
    let n = query.len().min(stored.len()).min(DIM);
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        let x = query[i];
        let y = stored[i] as f32;
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na <= 0.0 || nb <= 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Decode a stored int8 blob.
pub fn blob_to_i8(blob: &[u8]) -> Vec<i8> {
    blob.iter().map(|b| *b as i8).collect()
}

/// Encode an int8 vector for SQLite storage.
pub fn i8_to_blob(vector: &[i8]) -> Vec<u8> {
    vector.iter().map(|v| *v as u8).collect()
}

/// Convert an int8 vector to float for scoring.
pub fn i8_to_f32(vector: &[i8]) -> Vec<f32> {
    vector.iter().map(|v| *v as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vocabulary_is_loaded() {
        assert!(token_count() > 40_000);
        assert!(has_token("function"));
    }

    #[test]
    fn embeddings_are_deterministic_and_directional() {
        let a = embed_text("publish message to topic").unwrap();
        let b = embed_text("publish message to topic").unwrap();
        let c = embed_text("render pixel shader").unwrap();
        assert_eq!(a, b);
        assert!(cosine_i8(&a, &b) > 0.99);
        assert!(cosine_i8(&a, &a) >= cosine_i8(&a, &c));
    }

    #[test]
    fn unknown_tokens_do_not_panic() {
        assert!(embed_text("zzzznotatoken").is_none());
    }
}
