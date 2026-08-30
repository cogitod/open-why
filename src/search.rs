pub(crate) fn tokenize(question: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for w in question
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .filter(|w| !is_stopword(w))
    {
        if !seen.contains(&w) {
            seen.push(w);
        }
    }
    seen
}

/// Lexical term-overlap score: a subject hit is worth 5, a body hit 1.
/// Unbounded — db.rs normalizes it into a similarity proxy before blending.
pub(crate) fn score(words: &[String], subject: &str, body: &str) -> i64 {
    let subject = subject.to_lowercase();
    let body = body.to_lowercase();
    let mut s = 0i64;
    for w in words {
        if subject.contains(w) {
            s += 5;
        }
        if body.contains(w) {
            s += 1;
        }
    }
    s
}

fn is_stopword(w: &str) -> bool {
    matches!(
        w,
        "a" | "an"
            | "the"
            | "of"
            | "to"
            | "in"
            | "and"
            | "or"
            | "for"
            | "on"
            | "with"
            | "is"
            | "are"
            | "was"
            | "were"
            | "be"
            | "been"
            | "being"
            | "why"
            | "what"
            | "how"
            | "when"
            | "where"
            | "which"
            | "who"
            | "use"
            | "used"
            | "using"
            | "does"
            | "do"
            | "did"
            | "make"
            | "made"
            | "instead"
            | "than"
            | "that"
            | "this"
            | "these"
            | "those"
            | "it"
            | "its"
            | "we"
            | "our"
            | "your"
            | "my"
            | "i"
            | "not"
            | "but"
            | "as"
            | "at"
            | "by"
            | "from"
            | "into"
            | "about"
            | "should"
            | "would"
            | "could"
            | "will"
            | "can"
    )
}
