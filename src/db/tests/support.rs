use super::super::*;
pub(super) use crate::embed::{cosine, Embedder};
pub(super) use crate::store::Decision;

pub(super) struct FakeEmbedder;
impl Embedder for FakeEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(match text {
            "cat" | "feline" => vec![1.0, 0.0],
            "dog" => vec![0.0, 1.0],
            _ => vec![0.0, 0.0],
        })
    }
}

pub(super) fn decision(
    subject: &str,
    body: &str,
    importance: f64,
    embedding: Option<Vec<f32>>,
) -> Decision {
    Decision {
        sha: String::new(),
        author: String::new(),
        date: "2026-01-01T00:00:00Z".to_string(),
        updated_at: String::new(),
        subject: subject.to_string(),
        body: body.to_string(),
        source: String::new(),
        importance,
        kind: "decision".to_string(),
        access_count: 0,
        effectiveness: 0.5,
        embedding,
    }
}

pub(super) fn history_row(
    id: &str,
    successor: Option<&str>,
    scope: &str,
    content: &str,
) -> ExternalDecision {
    ExternalDecision {
        id: id.to_owned(),
        kind: "decision".to_owned(),
        title: format!("record {id}"),
        content: content.to_owned(),
        importance: 0.5,
        source: "synthetic".to_owned(),
        author: "tester".to_owned(),
        date: "2026-01-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: scope.to_owned(),
        valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
        valid_until: successor.map(|_| "2026-02-01T00:00:00Z".to_owned()),
        superseded_by: successor.map(str::to_owned),
        fact_key: None,
        git_refs: vec![GitRef {
            commit_hash: format!("commit-{id}"),
            commit_subject: format!("Apply {id}"),
        }],
    }
}

pub(super) static TMP_COUNTER: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub(super) fn temp_store() -> Store {
    // A monotonic counter guarantees a unique dir even when parallel tests collide on the
    // same nanosecond timestamp.
    let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!("open-why-test-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    Store::open_with_embedder_and_store_instance_id(
        &dir.join("t.db"),
        None,
        &format!("provider:test:{n}"),
    )
    .unwrap()
}

pub(super) fn evidence_identity(store: &Store, id: &str, scope: &str) -> EvidenceIdentity {
    match store.evidence_identity_in_scope(id, scope).unwrap() {
        EvidenceIdentityResolution::Ok { identity } => identity,
        resolution => panic!("expected evidence identity, got {resolution:?}"),
    }
}
