//! Small behavioral fixture, not an estimate of real-world recall or answer accuracy.
#[allow(dead_code)]
mod onboarding_support;
use open_why::{ExternalDecision, Store};
use serde_json::{json, Value};
fn fixture() -> Vec<ExternalDecision> {
    serde_json::from_str(include_str!("fixtures/public-beta/evidence.json")).unwrap()
}
fn check(store: &Store, semantic: bool) {
    store.import_external(&fixture()).unwrap();
    let ids = |q: &str, history| {
        store
            .search_records_with(q, &["notebook"], &[], 10, history)
            .unwrap()
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    };
    let hits = ids("SQLite offline notebook", false);
    assert_eq!(hits.first().map(String::as_str), Some("sqlite"));
    assert!(!hits.contains(&"foreign".into()));
    assert_eq!(hits, ids("SQLite offline notebook", false));
    assert!(!ids("cache expiry", false).contains(&"obsolete-cache".into()));
    assert!(ids("cache expiry", true).contains(&"obsolete-cache".into()));
    let current = serde_json::to_value(
        store
            .get_current_evidence_in_scope("obsolete-cache", "notebook")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(current["current_id"], "current-cache");
    let stale = serde_json::to_value(
        store
            .get_current_evidence_in_scope("expired-trial", "notebook")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(stale["code"], "expired_without_successor");
    let missing = serde_json::to_value(
        store
            .get_current_evidence_in_scope("no-encryption-decision", "notebook")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(missing["code"], "not_found");
    let disagreement = ids("compression recommendation", false);
    assert!(disagreement.contains(&"team-a".into()) && disagreement.contains(&"team-b".into()));
    assert!(!ids("temporary tracing", false).contains(&"expired-trial".into()));
    assert!(store
        .search_records("SQLite", &["empty"], &[], 10)
        .unwrap()
        .is_empty());
    if !semantic {
        assert!(ids("zygomorphicquasar", false).is_empty());
    }
    // Semantic candidates are similarities, never a claim of supporting evidence.
    let report: Value = json!({"mode":if semantic {"local-hybrid"} else {"lexical"},"sqlite_top":hits[0],"contradictory_records_preserved":true,"missing_exact_evidence":"not_found"});
    println!("{report}");
}
#[test]
fn public_lexical_evaluation() {
    let s = onboarding_support::Sandbox::new();
    let store = Store::open_with_store_instance_id(&s.0.join("evaluation.db"), "public:evaluation")
        .unwrap();
    check(&store, false);
    let other = Store::open_with_store_instance_id(&s.0.join("other.db"), "public:other").unwrap();
    assert!(other
        .search_records("SQLite", &["notebook"], &[], 10)
        .unwrap()
        .is_empty());
}
#[cfg(feature = "local-embeddings")]
#[test]
#[ignore = "requires pinned model; run scripts/evaluate-local.sh"]
fn public_local_embedding_evaluation() {
    let model = std::env::var("OPEN_WHY_EMBED_MODEL_PATH").expect("set pinned model path");
    let s = onboarding_support::Sandbox::new();
    let path = s.0.join("evaluation.db");
    let store = Store::open_with_embedder_and_store_instance_id(
        &path,
        Some(Box::new(
            open_why::LocalEmbedder::new(std::path::Path::new(&model)).unwrap(),
        )),
        "public:evaluation",
    )
    .unwrap();
    check(&store, true);
    let db = rusqlite::Connection::open(&path).unwrap();
    let vectors: Vec<String> = db
        .prepare("SELECT embedding FROM decisions")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(vectors.len(), fixture().len());
    for vector in vectors {
        let vector: Vec<f32> = serde_json::from_str(&vector).unwrap();
        assert_eq!(vector.len(), 384);
    }
    let (_, explanation) = store
        .search_records_explain("offline notebook", &["notebook"], &[], 1, false)
        .unwrap()
        .remove(0);
    assert!(explanation.semantic_rank.is_some());
    assert!(explanation.similarity > 0.0);
    println!("semantic ranking: {explanation:?}");
}
