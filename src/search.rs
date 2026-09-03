/// FTS5 stopwords — high-frequency tokens that match nearly the whole corpus.
/// Mirrors cogitod's `FTS_STOPWORDS` in `MemorySearchUtils.ts`, so the lexical arm
/// tokenizes identically to the TS engine it distills.
const FTS_STOPWORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "in", "on", "for", "and", "or", "is", "are", "was", "were", "be",
    "been", "with", "as", "at", "by", "it", "its", "this", "that", "these", "those", "from", "we",
    "you", "i", "can", "will", "do", "does", "how", "what", "why", "when", "our", "your", "my",
    "so", "if", "but", "not", "no", "all", "any", "into", "out", "up", "down", "about", "over",
];

/// Deduplicated, stopword-filtered search terms — the exact equivalent of cogitod's
/// `toSearchTerms`: lowercase, extract `/[a-z0-9_]+/` runs, dedupe, drop single-char
/// tokens and FTS5 stopwords. Note `_` is a token char (so `node_modules` stays one term),
/// unlike a naive non-alphanumeric split.
pub(crate) fn tokenize(question: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for term in question
        .to_lowercase()
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
    {
        if term.len() > 1 && !FTS_STOPWORDS.contains(&term) && !seen.iter().any(|s| s == term) {
            seen.push(term.to_string());
        }
    }
    seen
}
