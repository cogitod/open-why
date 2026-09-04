/// FTS5 stopwords — high-frequency tokens that match nearly the whole corpus.
const FTS_STOPWORDS: &[&str] = &[
    "a", "an", "the", "to", "of", "in", "on", "for", "and", "or", "is", "are", "was", "were", "be",
    "been", "with", "as", "at", "by", "it", "its", "this", "that", "these", "those", "from", "we",
    "you", "i", "can", "will", "do", "does", "how", "what", "why", "when", "our", "your", "my",
    "so", "if", "but", "not", "no", "all", "any", "into", "out", "up", "down", "about", "over",
];

/// Deduplicated, stopword-filtered search terms: lowercase, extract `/[a-z0-9_]+/`
/// runs, deduplicate, drop single-character
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_stopwords_but_keeps_non_stopword_short_words() {
        // "why", "do", "we", "for", "the" are stopwords; "use" is not.
        assert_eq!(
            tokenize("why do we use sqlite for the store"),
            vec!["use", "sqlite", "store"]
        );
    }

    #[test]
    fn underscore_keeps_a_token_together() {
        assert_eq!(
            tokenize("worktree node_modules symlink"),
            vec!["worktree", "node_modules", "symlink"]
        );
    }

    #[test]
    fn single_char_tokens_are_dropped() {
        assert_eq!(tokenize("a b sqlite"), vec!["sqlite"]);
    }

    #[test]
    fn dedupes_repeated_terms() {
        assert_eq!(
            tokenize("sqlite sqlite database"),
            vec!["sqlite", "database"]
        );
    }
}
