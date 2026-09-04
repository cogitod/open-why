use super::super::ranking::{
    recency_decay, recency_weight_for, RECENCY_BOOST, RECENCY_DECAY_FLOOR, RECENCY_SUPPRESS,
};
use super::super::*;
use super::support::*;

#[test]
fn cosine_is_bounded_and_symmetric() {
    assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
    assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
    assert_eq!(cosine(&[1.0, 0.0], &[]), 0.0);
}

#[test]
fn lexical_first_without_query_embedding() {
    let rows = vec![
        decision("sqlite local record", "single file", 0.5, None),
        decision("postgres", "row level security", 0.5, None),
    ];
    // FTS5 lexical arm: only row 0 matches "sqlite".
    let ranked = rank("sqlite", None, rows, vec![0], 1700000000, 10);
    assert_eq!(ranked[0].subject, "sqlite local record");
}

#[test]
fn semantic_similarity_surfaces_a_row_with_no_lexical_overlap() {
    // "feline" shares no token with "cat", but its embedding matches. Semantic
    // similarity must rank it first and must not require a lexical hit.
    let rows = vec![
        decision(
            "feline",
            "a small domesticated animal",
            0.5,
            Some(vec![1.0, 0.0]),
        ),
        decision("dog", "a loyal companion", 0.5, Some(vec![0.0, 1.0])),
    ];
    let q = FakeEmbedder.embed("cat").unwrap();
    // No lexical overlap: the FTS5 arm returns nothing, the semantic arm must carry.
    let ranked = rank("cat", Some(&q), rows, Vec::new(), 1700000000, 10);
    assert_eq!(ranked[0].subject, "feline");
}

#[test]
fn missing_embedding_falls_back_to_lexical_proxy() {
    // A row with no embedding still ranks via the lexical (FTS5) arm and is not dropped.
    let rows = vec![decision("postgres", "row level security", 0.5, None)];
    let ranked = rank("postgres", Some(&[1.0, 0.0]), rows, vec![0], 1700000000, 10);
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].subject, "postgres");
}

#[test]
fn recency_decay_floors_at_0_3() {
    // Age must never bury a correct answer at zero; the decay asymptotes at 0.3.
    assert!((recency_decay(0.0, 7.0) - 1.0).abs() < 1e-9);
    assert!((recency_decay(1_000.0, 7.0) - RECENCY_DECAY_FLOOR).abs() < 1e-9);
    // Non-positive half-life returns the floor rather than dividing by zero.
    assert_eq!(recency_decay(10.0, 0.0), RECENCY_DECAY_FLOOR);
}

#[test]
fn recency_decay_uses_spaced_repetition_stability() {
    // A frequently-accessed memory decays slower than its raw age would suggest:
    // stability = half-life × (1 + ln(1 + access_count)).
    let age = 20.0;
    let flat = recency_decay(age, 7.0); // access_count = 0
    let stability = 7.0 * (1.0 + (1.0 + 100.0f64).ln());
    let spaced = recency_decay(age, stability);
    assert!(spaced > flat, "spaced={spaced} should exceed flat={flat}");
}

#[test]
fn query_conditional_recency_weights() {
    assert!((recency_weight_for("the latest lane policy") - RECENCY_BOOST).abs() < 1e-9);
    assert!((recency_weight_for("how it used to work") - RECENCY_SUPPRESS).abs() < 1e-9);
    assert!((recency_weight_for("worktree corruption") - 1.0).abs() < 1e-9);
}

#[test]
fn fts5_lexical_narrow_then_broad_prefers_all_terms() {
    // FTS5 lexical arm: for a two-term query, the all-terms (AND) arm wins when it yields
    // >= min(limit, 5) rows, so the partial-match row is excluded from the lexical arm.
    let store = temp_store();
    for i in 0..5 {
        store
            .capture(
                &decision(&format!("sqlite postgres {i}"), "both", 0.5, None),
                "global",
                None,
            )
            .unwrap();
    }
    store
        .capture(
            &decision("sqlite sqlite sqlite sqlite", "no second token", 0.5, None),
            "global",
            None,
        )
        .unwrap();
    let hits = store
        .search("sqlite postgres", &["global"], &[], 10)
        .unwrap();
    assert_eq!(hits.len(), 5);
    assert!(hits.iter().all(|h| h.subject.contains("postgres")));
}

#[test]
fn fts5_lexical_narrow_then_broad_falls_back() {
    // Only one all-terms row (< narrow floor), so the arm broadens to OR and the
    // partial-match row still surfaces.
    let store = temp_store();
    store
        .capture(
            &decision("sqlite postgres", "both", 0.5, None),
            "global",
            None,
        )
        .unwrap();
    store
        .capture(
            &decision("sqlite", "only one term", 0.5, None),
            "global",
            None,
        )
        .unwrap();
    let hits = store
        .search("sqlite postgres", &["global"], &[], 10)
        .unwrap();
    assert_eq!(hits.len(), 2);
}

#[test]
fn fts5_lexical_orders_multi_term_match_first() {
    // The row matching both query terms must outrank the row matching only one. FTS5 bm25()
    // handles idf and length normalisation natively. SQLite owns this behavior.
    let store = temp_store();
    store
        .capture(
            &decision("worktree long", &("node_modules ".repeat(300)), 0.5, None),
            "global",
            None,
        )
        .unwrap();
    store
        .capture(
            &decision("worktree corruption", "corruption", 0.5, None),
            "global",
            None,
        )
        .unwrap();
    let hits = store
        .search("worktree corruption", &["global"], &[], 10)
        .unwrap();
    assert_eq!(hits[0].subject, "worktree corruption");
}

#[test]
fn active_search_excludes_future_and_expired_temporal_records() {
    let store = temp_store();
    let insert = |id: &str, from: &str, until: Option<&str>| {
        let mut row = history_row(id, None, "scope-a", "temporal sentinel");
        row.title = "temporal sentinel".to_owned();
        row.valid_from = Some(from.to_owned());
        row.valid_until = until.map(str::to_owned);
        store.import_external(&[row]).unwrap();
    };
    insert("current", "2000-01-01T00:00:00Z", None);
    insert("future", "2999-01-01T00:00:00Z", None);
    insert(
        "expired",
        "2000-01-01T00:00:00Z",
        Some("2001-01-01T00:00:00Z"),
    );

    let active = store
        .search_records("temporal sentinel", &["scope-a"], &[], 10)
        .unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "current");

    let historical = store
        .search_records_with("temporal sentinel", &["scope-a"], &[], 10, true)
        .unwrap();
    assert_eq!(historical.len(), 3);
}

#[test]
fn explain_reports_components_and_drops() {
    let store = temp_store();
    for i in 0..3 {
        store
            .capture(
                &decision(&format!("sqlite postgres {i}"), "both terms", 0.5, None),
                "global",
                None,
            )
            .unwrap();
    }
    let explained = store
        .search_records_explain("sqlite postgres", &["global"], &[], 3, false)
        .unwrap();
    assert_eq!(explained.len(), 3);
    assert!(explained.iter().all(|(_, e)| e.lexical_rank.is_some()));
    assert!(explained.iter().all(|(_, e)| e.semantic_rank.is_none()));
    assert!(explained.iter().all(|(_, e)| e.rrf_score > 0.0));
    let (results, drops) = store
        .search_records_drops("sqlite postgres", &["global"], &[], 1, false, 5)
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(drops.len(), 2);
}
