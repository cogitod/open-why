//! Post-fusion relevance gate. Constants and word lists are calibrated against retrieval
//! fixtures and live as named values rather than magic numbers. Do not retune them without
//! representative regression evidence.

/// Calibrated 2026-08-12 against `retrieval-threshold-calibration.v2.json` and two further
/// fixtures.
pub const RAG_UTILITY_THRESHOLD: f64 = 0.3825;
/// Similarity at or above this bypasses the lexical check entirely.
pub const RAG_SEMANTIC_BYPASS: f64 = 0.35;
/// A nonzero similarity below this is refused outright.
pub const SIMILARITY_FLOOR: f64 = 0.34;

const LEXICAL_COMMON_WEIGHT: f64 = 0.20;

/// Query words that carry no topic; dropped from the query side only.
const LEXICAL_STOPWORDS: &[&str] = &[
    "the", "and", "for", "are", "was", "were", "been", "being", "have", "has", "had", "does",
    "did", "doing", "you", "your", "our", "its", "their", "this", "that", "these", "those", "what",
    "which", "who", "whom", "whose", "when", "where", "why", "how", "can", "could", "should",
    "would", "will", "shall", "may", "might", "must", "with", "from", "into", "onto", "about",
    "over", "under", "between", "through", "during", "before", "after", "above", "below", "again",
    "once", "here", "there", "then", "than", "too", "very", "just", "but", "because", "while",
    "until", "also", "get", "got", "use", "used", "using", "via", "per",
];

/// Everyday-world vocabulary, weighted down as admission evidence — a technical term is
/// evidence in a technical corpus, an everyday-world noun almost never is.
const LEXICAL_COMMON_TERMS: &[&str] = &[
    "water",
    "air",
    "heat",
    "wet",
    "cool",
    "food",
    "bread",
    "tea",
    "coffee",
    "milk",
    "sugar",
    "salt",
    "meal",
    "dish",
    "tree",
    "flower",
    "grass",
    "animal",
    "bird",
    "fish",
    "plant",
    "garden",
    "house",
    "room",
    "door",
    "road",
    "city",
    "town",
    "country",
    "weather",
    "rain",
    "snow",
    "wind",
    "moon",
    "sky",
    "sea",
    "ocean",
    "river",
    "mountain",
    "sand",
    "wood",
    "cloth",
    "music",
    "song",
    "sport",
    "movie",
    "film",
    "photo",
    "good",
    "better",
    "best",
    "bad",
    "worse",
    "worst",
    "nice",
    "great",
    "poor",
    "small",
    "large",
    "big",
    "little",
    "tiny",
    "huge",
    "easy",
    "difficult",
    "heavy",
    "loud",
    "quiet",
    "bright",
    "happy",
    "much",
    "many",
    "more",
    "most",
    "less",
    "least",
    "few",
    "several",
    "want",
    "love",
    "hate",
    "feel",
    "seem",
    "eat",
    "drink",
    "walk",
    "buy",
    "sell",
    "die",
    "wear",
];

/// Split on Unicode letter/digit runs, not whitespace, so punctuation never glues to a token.
/// Extract Unicode alphanumeric runs; tokens of length two or less are dropped.
fn lexical_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for c in text.chars() {
        if c.is_alphabetic() || c.is_numeric() {
            current.extend(c.to_lowercase());
        } else if !current.is_empty() {
            if current.chars().count() > 2 {
                tokens.push(std::mem::take(&mut current));
            } else {
                current.clear();
            }
        }
    }
    if current.chars().count() > 2 {
        tokens.push(current);
    }
    tokens
}

/// How much of the query's evidence does this passage actually contain? Weighted term
/// coverage, not occurrence count.
pub fn rag_composite_score(query: &str, content: &str) -> f64 {
    let query_all = lexical_tokens(query);
    let meaningful: Vec<&str> = query_all
        .iter()
        .map(String::as_str)
        .filter(|t| !LEXICAL_STOPWORDS.contains(t))
        .collect();
    let query_tokens: std::collections::HashSet<&str> = if !meaningful.is_empty() {
        meaningful.into_iter().collect()
    } else {
        query_all.iter().map(String::as_str).collect()
    };
    if query_tokens.is_empty() {
        return 1.0;
    }

    let content_all = lexical_tokens(content);
    let matched: std::collections::HashSet<&str> = content_all
        .iter()
        .map(String::as_str)
        .filter(|t| query_tokens.contains(t))
        .collect();

    let weigh = |terms: &std::collections::HashSet<&str>| -> f64 {
        terms
            .iter()
            .map(|t| {
                if LEXICAL_COMMON_TERMS.contains(t) {
                    LEXICAL_COMMON_WEIGHT
                } else {
                    1.0
                }
            })
            .sum()
    };

    let rel = weigh(&matched) / weigh(&query_tokens);
    let sup = if content.chars().count() > 20 {
        0.7
    } else {
        0.4
    };
    let use_ = (rel + sup) / 2.0;
    rel * 0.4 + sup * 0.3 + use_ * 0.3
}

/// `relevanceVerdict`'s lexical half (`MemoryRelevanceGate.ts:48`): a short raw `content` is
/// always fully admissible on lexical grounds; anything longer is scored against `lexical_text`
/// (title+content combined, when a title exists).
pub fn lexical_score(query: &str, content: &str, lexical_text: &str) -> f64 {
    if content.chars().count() < 50 {
        1.0
    } else {
        rag_composite_score(query, lexical_text)
    }
}

/// `scoredRelevanceVerdict` (`MemoryRelevanceGate.ts:29`): reject a real-but-weak vector score,
/// admit a strong one outright, otherwise fall back to lexical utility. `similarity == 0.0` is
/// "no vector signal" (lexical-only candidate), not "orthogonal to the query", and stays eligible.
pub fn passes(similarity: f64, lexical_score: f64) -> bool {
    if similarity > 0.0 && similarity < SIMILARITY_FLOOR {
        return false;
    }
    if similarity >= RAG_SEMANTIC_BYPASS {
        return true;
    }
    lexical_score >= RAG_UTILITY_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_content_is_always_lexically_admissible() {
        assert_eq!(lexical_score("anything at all", "short", "short"), 1.0);
    }

    #[test]
    fn lexical_only_candidate_needs_utility_not_bypass() {
        // similarity == 0.0 must not be refused by the floor, and must not get a free pass
        // from the bypass branch either — it lives or dies on lexical_score alone.
        assert!(passes(0.0, RAG_UTILITY_THRESHOLD));
        assert!(!passes(0.0, RAG_UTILITY_THRESHOLD - 0.01));
    }

    #[test]
    fn weak_nonzero_similarity_is_refused() {
        assert!(!passes(SIMILARITY_FLOOR - 0.01, 1.0));
    }

    #[test]
    fn strong_similarity_bypasses_lexical_check() {
        assert!(passes(RAG_SEMANTIC_BYPASS, 0.0));
    }

    #[test]
    fn common_term_match_is_weighted_down() {
        // "water" is a common term (weight 0.20); a domain term of equal query length is not.
        let common = rag_composite_score("optimal water temperature", "notes about water only");
        let domain =
            rag_composite_score("optimal worktree temperature", "notes about worktree only");
        assert!(domain > common);
    }

    #[test]
    fn stopword_only_query_falls_back_to_all_tokens() {
        // "why does this" is entirely stopwords; scoring must not divide by zero.
        let score =
            rag_composite_score("why does this", "some unrelated content over twenty chars");
        assert!((0.0..=1.0).contains(&score));
    }
}
