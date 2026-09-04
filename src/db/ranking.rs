use super::{iso_to_epoch, Decision, RankExplanation};

/// RRF fusion constant (Cormack et al. 2009).
const RRF_K: f64 = 60.0;
/// BM25 leads the inline fusion (arXiv 2605.15184, Table 1).
const BM25_WEIGHT: f64 = 1.5;
/// Calibrated hybrid rerank weights (similarity / importance / effectiveness).
const RERANK_W_SIM: f64 = 0.65;
const RERANK_W_IMPORTANCE: f64 = 0.25;
const RERANK_W_EFFECTIVENESS: f64 = 0.10;
/// Floor under recency decay: an old-but-best match
/// must stay reachable rather than being buried to zero by age alone.
pub(super) const RECENCY_DECAY_FLOOR: f64 = 0.3;
const RECENCY_HALF_LIFE_DAYS: f64 = 7.0;
const RECENCY_HALF_LIFE_DECISION_DAYS: f64 = 2.0;
/// Query-conditional recency weighting.
pub(super) const RECENCY_BOOST: f64 = 2.5;
pub(super) const RECENCY_SUPPRESS: f64 = 0.3;

/// Ebbinghaus recency decay with a floor: `2^(-age/halfLife)`, clamped at RECENCY_DECAY_FLOOR.
pub(super) fn recency_decay(age_days: f64, half_life_days: f64) -> f64 {
    if half_life_days <= 0.0 || half_life_days.is_nan() || !age_days.is_finite() {
        return RECENCY_DECAY_FLOOR;
    }
    (2.0f64.powf(-age_days.max(0.0) / half_life_days)).max(RECENCY_DECAY_FLOOR)
}

fn contains_word(haystack: &str, word: &str) -> bool {
    haystack
        .split(|c: char| !c.is_alphanumeric())
        .any(|t| t == word)
}

/// Query-conditional recency multiplier. Word-boundary match (via tokenization) so `now` does
/// not match inside `snow`, `as of` / `used to` are phrase matches.
pub(super) fn recency_weight_for(query: &str) -> f64 {
    let lower = query.to_lowercase();
    const CURRENT_WORDS: &[&str] = &["current", "currently", "latest", "now", "today", "present"];
    const PAST_WORDS: &[&str] = &[
        "originally",
        "first",
        "initial",
        "initially",
        "previously",
        "formerly",
        "history",
        "historical",
        "past",
        "earlier",
        "before",
    ];
    const CURRENT_PHRASES: &[&str] = &["as of", "most recent", "up to date", "up-to-date"];
    const PAST_PHRASES: &[&str] = &["used to"];
    if CURRENT_PHRASES.iter().any(|p| lower.contains(p))
        || CURRENT_WORDS.iter().any(|w| contains_word(&lower, w))
    {
        return RECENCY_BOOST;
    }
    if PAST_PHRASES.iter().any(|p| lower.contains(p))
        || PAST_WORDS.iter().any(|w| contains_word(&lower, w))
    {
        return RECENCY_SUPPRESS;
    }
    1.0
}

/// The fields `rank_by` needs per row. References borrow from the row for the duration of the
/// scoring pass only; only primitives are copied out.
pub(super) struct RankRow<'a> {
    pub(super) importance: f64,
    pub(super) kind: &'a str,
    pub(super) date: &'a str,
    pub(super) updated_at: Option<&'a str>,
    pub(super) access_count: i64,
    pub(super) effectiveness: f64,
    pub(super) embedding: Option<&'a [f32]>,
    pub(super) title: &'a str,
    pub(super) content: &'a str,
}

/// Hybrid rerank using reciprocal-rank fusion of a
/// semantic arm (sorted by hybrid score) and a lexical arm (the FTS5 `bm25()` order supplied by
/// the caller, already narrow-then-broad), then slice. Recency enters through the semantic arm's
/// hybrid score. It is floored so age cannot bury a best match and never acts as a multiplicative gate on
/// the fused score.
pub(super) fn rank(
    query: &str,
    query_embedding: Option<&[f32]>,
    rows: Vec<Decision>,
    lexical_order: Vec<usize>,
    now: i64,
    limit: usize,
) -> Vec<Decision> {
    rank_by(
        query,
        query_embedding,
        rows,
        lexical_order,
        now,
        limit,
        |d| RankRow {
            importance: d.importance,
            kind: &d.kind,
            date: &d.date,
            updated_at: if d.updated_at.is_empty() {
                None
            } else {
                Some(&d.updated_at)
            },
            access_count: d.access_count,
            effectiveness: d.effectiveness,
            embedding: d.embedding.as_deref(),
            title: &d.subject,
            content: &d.body,
        },
    )
    .0
}

pub(super) fn rank_by<T>(
    query: &str,
    query_embedding: Option<&[f32]>,
    rows: Vec<T>,
    lexical_order: Vec<usize>,
    now: i64,
    limit: usize,
    fields: impl Fn(&T) -> RankRow<'_>,
) -> (Vec<T>, Vec<RankExplanation>) {
    let recency_mult = recency_weight_for(query);

    // Semantic score capsule. The lexical arm is the native FTS5 bm25() order supplied by the
    // caller; this computes only what the semantic arm needs.
    struct Capsule {
        sim: f64,
        embedded: bool,
        importance: f64,
        age_days: f64,
        half_life: f64,
        access_count: i64,
        effectiveness: f64,
        lexical_gate_score: f64,
    }
    let has_query_emb = query_embedding.is_some();
    let capsules: Vec<Capsule> = rows
        .iter()
        .map(|d| {
            let f = fields(d);
            let (sim, embedded) = match (query_embedding, f.embedding) {
                (Some(q), Some(e)) => (crate::embed::cosine(q, e) as f64, true),
                _ => (0.0, false),
            };
            let age_src = f.updated_at.unwrap_or(f.date);
            let age_days = iso_to_epoch(age_src)
                .map(|ep| ((now - ep) as f64 / 86_400.0).max(0.0))
                .unwrap_or(0.0);
            let half_life = if f.kind == "decision" {
                RECENCY_HALF_LIFE_DECISION_DAYS
            } else {
                RECENCY_HALF_LIFE_DAYS
            };
            let lexical_text = if f.title.is_empty() {
                f.content.to_string()
            } else {
                format!("{}\n{}", f.title, f.content)
            };
            let lexical_gate_score =
                crate::relevance::lexical_score(query, f.content, &lexical_text);
            Capsule {
                sim,
                embedded,
                importance: f.importance,
                age_days,
                half_life,
                access_count: f.access_count,
                effectiveness: f.effectiveness,
                lexical_gate_score,
            }
        })
        .collect();

    let hybrid = |c: &Capsule| -> f64 {
        // Ebbinghaus with spaced-repetition stability: more accesses widen the half-life, so a
        // frequently-surfaced memory decays slower than its raw age would suggest.
        let stability = c.half_life * (1.0 + (1.0 + c.access_count as f64).ln());
        let decay = recency_decay(c.age_days, stability);
        (RERANK_W_SIM * c.sim
            + RERANK_W_IMPORTANCE * c.importance
            + RERANK_W_EFFECTIVENESS * c.effectiveness)
            * decay
            * recency_mult
    };

    let n = capsules.len();

    // Semantic arm: keep only the nearest-by-cosine rows (the semantic
    // neighbourhood), then order THAT set by hybrid score. Ordering the whole corpus by hybrid
    // score would let recency/importance crowd out semantically-far rows before fusion.
    let semantic_order: Vec<usize> = if has_query_emb {
        let mut embedded: Vec<usize> = (0..n).filter(|&i| capsules[i].embedded).collect();
        embedded.sort_by(|&a, &b| {
            capsules[b]
                .sim
                .partial_cmp(&capsules[a].sim)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let k = (limit.saturating_mul(30)).max(256);
        embedded.truncate(k);
        embedded.sort_by(|&a, &b| {
            hybrid(&capsules[b])
                .partial_cmp(&hybrid(&capsules[a]))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        embedded
    } else {
        Vec::new()
    };

    // Reciprocal rank fusion.
    let mut scores = vec![0.0f64; n];
    let mut semantic_rank: Vec<Option<usize>> = vec![None; n];
    let mut lexical_rank: Vec<Option<usize>> = vec![None; n];
    for (rank, &i) in semantic_order.iter().enumerate() {
        scores[i] += 1.0 / (RRF_K + rank as f64 + 1.0);
        semantic_rank[i] = Some(rank);
    }
    for (rank, &i) in lexical_order.iter().enumerate() {
        scores[i] += BM25_WEIGHT / (RRF_K + rank as f64 + 1.0);
        lexical_rank[i] = Some(rank);
    }

    if std::env::var("OPEN_WHY_DEBUG_RANK").is_ok() {
        eprintln!(
            "[rank] query={query} semantic={} lexical={}",
            semantic_order.len(),
            lexical_order.len()
        );
        for (rank, &i) in semantic_order.iter().take(12).enumerate() {
            let c = &capsules[i];
            eprintln!(
                "  SEM[{rank}] sim={:.3} imp={:.2} age={:.0} fused={:.5}",
                c.sim, c.importance, c.age_days, scores[i]
            );
        }
        for (rank, &i) in lexical_order.iter().take(12).enumerate() {
            let c = &capsules[i];
            eprintln!(
                "  LEX[{rank}] sim={:.3} lex_gate={:.4} fused={:.5}",
                c.sim, c.lexical_gate_score, scores[i]
            );
        }
    }

    // Fused candidate set = union of the two arms, best-fused first.
    let mut order: Vec<usize> = semantic_order
        .iter()
        .copied()
        .chain(lexical_order.iter().copied())
        .collect();
    order.sort_unstable();
    order.dedup();
    // Post-fusion relevance gate: drop candidates
    // that cleared BM25/RRF fusion but are not actually relevant to the query, before the
    // final score sort, so a filtered-out noise row can't block a genuine match from the
    // top-N slice. Must run on the full fused set, not just the eventual top `limit`.
    order.retain(|&i| crate::relevance::passes(capsules[i].sim, capsules[i].lexical_gate_score));
    order.sort_by(|&a, &b| {
        scores[b]
            .partial_cmp(&scores[a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    order.truncate(limit);

    let mut row_vec: Vec<Option<T>> = rows.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(order.len());
    let mut explanations = Vec::with_capacity(order.len());
    for i in order {
        if let Some(r) = row_vec[i].take() {
            let c = &capsules[i];
            let stability = c.half_life * (1.0 + (1.0 + c.access_count as f64).ln());
            let decay = recency_decay(c.age_days, stability);
            out.push(r);
            explanations.push(RankExplanation {
                similarity: c.sim,
                importance: c.importance,
                effectiveness: c.effectiveness,
                age_days: c.age_days,
                recency_decay: decay,
                hybrid_score: hybrid(c),
                semantic_rank: semantic_rank[i],
                lexical_rank: lexical_rank[i],
                rrf_score: scores[i],
            });
        }
    }
    (out, explanations)
}
