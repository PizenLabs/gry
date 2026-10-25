//! Identifier and text tokenization shared by search, MinHash and embeddings.

/// Split an identifier into lowercase tokens.
///
/// Handles camelCase, PascalCase, snake_case, kebab-case, dots, `::`, and
/// acronym boundaries (`HTTPServer` -> `http`, `server`).
pub fn split_identifier(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let chars: Vec<char> = input.chars().collect();
    for (i, &ch) in chars.iter().enumerate() {
        if !ch.is_alphanumeric() {
            if !current.is_empty() {
                out.push(current.to_ascii_lowercase());
                current.clear();
            }
            continue;
        }
        if ch.is_uppercase() && !current.is_empty() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|c| c.is_lowercase());
            if prev.is_lowercase() || prev.is_ascii_digit() || (prev.is_uppercase() && next_lower) {
                out.push(current.to_ascii_lowercase());
                current.clear();
            }
        }
        current.push(ch);
    }
    if !current.is_empty() {
        out.push(current.to_ascii_lowercase());
    }
    out.retain(|t| !t.is_empty());
    out
}

/// Split free text into identifier tokens, dropping very short noise words.
pub fn split_text(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in input.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if raw.is_empty() {
            continue;
        }
        for token in split_identifier(raw) {
            if token.len() >= 2 || token.chars().all(|c| c.is_ascii_digit()) {
                out.push(token);
            }
        }
    }
    out
}

/// Normalize a config key for exact-name matching: lowercase, remove `_`, `-`,
/// and dots. `max_retry_count` and `maxRetryCount` both become `maxretrycount`.
pub fn normalize_config_key(input: &str) -> String {
    input
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Build token shingles (n-grams) for structural fingerprints.
pub fn shingles(tokens: &[String], n: usize) -> Vec<String> {
    if tokens.is_empty() {
        return Vec::new();
    }
    if tokens.len() < n {
        return tokens.to_vec();
    }
    let mut out = Vec::with_capacity(tokens.len() - n + 1);
    for window in tokens.windows(n) {
        out.push(window.join("\u{1f}"));
    }
    out
}

/// Stable token hash used by MinHash and random indexing.
pub fn stable_hash(token: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(token.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camel_and_snake() {
        assert_eq!(split_identifier("HTTPServer"), vec!["http", "server"]);
        assert_eq!(split_identifier("get_userID"), vec!["get", "user", "id"]);
        assert_eq!(
            split_identifier("XMLHttpRequest"),
            vec!["xml", "http", "request"]
        );
        assert_eq!(
            split_identifier("max_retry_count"),
            vec!["max", "retry", "count"]
        );
        assert_eq!(split_identifier("a.b::c"), vec!["a", "b", "c"]);
    }

    #[test]
    fn config_keys() {
        assert_eq!(normalize_config_key("maxRetryCount"), "maxretrycount");
        assert_eq!(normalize_config_key("MAX_RETRY_COUNT"), "maxretrycount");
        assert_eq!(normalize_config_key("max-retry.count"), "maxretrycount");
    }

    #[test]
    fn text_tokens() {
        let t = split_text("Publish to the ProductCatalog service");
        assert!(t.contains(&"publish".to_string()));
        assert!(t.contains(&"product".to_string()));
    }
}
