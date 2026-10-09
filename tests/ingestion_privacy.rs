#[allow(dead_code)]
mod onboarding_support;
use onboarding_support::{Sandbox, Server};
use open_why::{Decision, Embedder, ExternalDecision, SensitiveDataRejected, Store};
use serde_json::json;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn marker() -> String {
    format!("{}{}", "ghp_", "A".repeat(36))
}
fn row() -> ExternalDecision {
    serde_json::from_str::<Vec<ExternalDecision>>(include_str!(
        "fixtures/public-beta/evidence.json"
    ))
    .unwrap()
    .remove(0)
}
struct Spy(Arc<AtomicUsize>);
impl Embedder for Spy {
    fn embed(&self, _: &str) -> anyhow::Result<Vec<f32>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![1.0, 0.0])
    }
}
#[test]
fn credentials_are_rejected_before_storage_retirement_or_embedding() {
    let s = Sandbox::new();
    let path = s.0.join("privacy.db");
    let calls = Arc::new(AtomicUsize::new(0));
    let store = Store::open_with_embedder_and_store_instance_id(
        &path,
        Some(Box::new(Spy(calls.clone()))),
        "test:privacy",
    )
    .unwrap();
    let safe = Decision {
        subject: "Use SQLite".into(),
        body: "Internal rationale remains useful locally".into(),
        ..Decision::default()
    };
    let predecessor = store.capture(&safe, "scope", None).unwrap();
    let bad = Decision {
        body: marker(),
        ..safe.clone()
    };
    for result in [
        store.capture(&bad, "scope", Some(&predecessor)),
        store.capture_external(&bad, "scope", "external", None, None, Some(&predecessor)),
    ] {
        let error = result.unwrap_err();
        assert!(error.downcast_ref::<SensitiveDataRejected>().is_some());
        assert!(!error.to_string().contains(&marker()));
    }
    assert!(store.get_record(&predecessor).unwrap().is_some());
    assert_eq!(store.count_for_scope("scope").unwrap(), 1);
    assert!(store
        .import_decisions("scope", &[safe.clone(), bad])
        .is_err());
    for field in ["content", "source", "author", "tags", "id"] {
        let mut bad = serde_json::to_value(row()).unwrap();
        bad[field] = json!(marker());
        let bad: ExternalDecision = serde_json::from_value(bad).unwrap();
        let batch = [row(), bad];
        assert!(store.import_external(&batch).is_err());
        assert!(store.import_external_sealed(&batch).is_err());
    }
    let mut escaped_tags = row();
    escaped_tags.tags = Some(format!("[\"\\u0067{}\"]", &marker()[1..]));
    assert!(store.import_external(&[escaped_tags]).is_err());
    assert_eq!(store.count_for_scope(&row().scope).unwrap(), 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(store.link_git(&predecessor, "hash", &marker()).is_err());
    assert!(store.rescope("scope", &marker()).is_err());
    store.search(&marker(), &["scope"], &[], 1).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0, "query reached embedder");
    let remote = open_why::HttpEmbedder::new("http://127.0.0.1:9".into(), "model".into(), None);
    let error = remote.embed(&marker()).unwrap_err();
    assert!(
        error.downcast_ref::<SensitiveDataRejected>().is_some(),
        "must reject before transport"
    );
    // Existing safe imports and embedding remain functional.
    store.import_external(&[row()]).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(store);
    for suffix in ["", "-wal", "-shm"] {
        if let Ok(bytes) = std::fs::read(format!("{}{suffix}", path.display())) {
            assert!(!bytes
                .windows(marker().len())
                .any(|window| window == marker().as_bytes()));
        }
    }
}
#[test]
fn cli_mcp_and_local_git_index_do_not_persist_detected_credentials() {
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    let out = s
        .command()
        .env("OPEN_WHY_DB", &db)
        .env("OPEN_WHY_STORE_INSTANCE_ID", "test:privacy")
        .args(["capture", "--title", "Privacy", "--content", &marker()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains(&marker()));
    let mut server = Server::spawn(
        &s,
        &json!({"command":env!("CARGO_BIN_EXE_why"),"args":["serve"],
        "env":{"OPEN_WHY_DB":db,"OPEN_WHY_STORE_INSTANCE_ID":"test:privacy"}}),
    );
    let response = server.request(
        "tools/call",
        json!({"name":"open-why_capture",
        "arguments":{"title":"Privacy","content":marker(),"scope":"scope"}}),
    );
    assert_eq!(response["result"]["isError"], true);
    assert!(response.to_string().contains("sensitive_data"));
    assert!(!response.to_string().contains(&marker()));
    let mut git = std::process::Command::new("git");
    s.isolate(&mut git);
    assert!(git
        .args(["-C", s.0.to_str().unwrap(), "init", "--quiet"])
        .status()
        .unwrap()
        .success());
    let mut git = std::process::Command::new("git");
    s.isolate(&mut git);
    let out = git
        .args([
            "-C",
            s.0.to_str().unwrap(),
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgSign=false",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            &marker(),
        ])
        .output()
        .unwrap();
    onboarding_support::success(&out);
    let out = s
        .command()
        .env("OPEN_WHY_DB", &db)
        .env("OPEN_WHY_STORE_INSTANCE_ID", "test:privacy")
        .arg("init")
        .arg(&s.0)
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(!String::from_utf8_lossy(&out.stderr).contains(&marker()));
    let store = Store::open_with_store_instance_id(&db, "test:privacy").unwrap();
    assert_eq!(store.count_for_scope("scope").unwrap(), 0);
    assert_eq!(store.count_for_scope(s.0.to_str().unwrap()).unwrap(), 0);
    // Decline network cloning before Git or any cache creation.
    let out = s
        .command()
        .args(["init", "https://fixture.invalid/project.git"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("automatic remote cloning is disabled"));
    assert!(!s.0.join(".cache/open-why/repos").exists());
}
#[test]
fn detector_covers_common_credentials_without_classifying_private_prose() {
    let cases = [
        marker(),
        format!("{}{}", "sk-proj-", "a".repeat(30)),
        format!("{}{}", "AKIA", "A".repeat(16)),
        format!("{}{}", "xoxb-", "1234567890-1234567890"),
        format!("-----BEGIN {}-----", "PRIVATE KEY"),
        format!("password={}", "synthetic-value"),
        format!("password={}", "x"),
        format!("password         =         {}", "x"),
        format!("OPEN_WHY_EMBED_API_KEY={}", "synthetic-value"),
        format!("Authorization: Bearer {}", "synthetic-value"),
        format!("https://user:{}@fixture.invalid", "synthetic-value"),
        r#" {"\u0070assword":"x"}"#.into(),
    ];
    for value in cases {
        assert!(open_why::privacy::check_text(&value).is_err());
    }
    for value in [
        "Discuss password rotation",
        "A basic engineering decision about Bearer authentication",
        "Use $API_KEY from the environment",
        "Internal roadmap discussion",
        "password=<redacted>",
        "OPEN_WHY_EMBED_API_KEY=...",
    ] {
        assert!(open_why::privacy::check_text(value).is_ok());
    }
}
