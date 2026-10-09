#[allow(dead_code)]
mod onboarding_support;
use onboarding_support::{success, Sandbox};
use std::{
    path::Path,
    process::{Command, Output},
};

fn git(s: &Sandbox, path: &Path, args: &[&str]) {
    let mut command = Command::new("git");
    s.isolate(&mut command);
    let out = command.arg("-C").arg(path).args(args).output().unwrap();
    success(&out);
}
fn index(s: &Sandbox, owner: &str) -> Output {
    // Public-looking URLs are redirected to synthetic local Git fixtures; no network access.
    s.command()
        .env("OPEN_WHY_DB", s.0.join("store.db"))
        .env("OPEN_WHY_STORE_INSTANCE_ID", "test:repositories")
        .env("GIT_CONFIG_COUNT", "1")
        .env(
            "GIT_CONFIG_KEY_0",
            format!("url.file://{}/.insteadOf", s.0.join("remotes").display()),
        )
        .env("GIT_CONFIG_VALUE_0", "https://fixture.invalid/")
        .arg("init")
        .arg(format!("https://fixture.invalid/{owner}/project.git"))
        .output()
        .unwrap()
}
#[test]
fn same_basename_remote_repositories_never_share_checkout_or_evidence_scope() {
    let s = Sandbox::new();
    for owner in ["alpha", "beta"] {
        let path = s.0.join(format!("remotes/{owner}/project.git"));
        std::fs::create_dir_all(&path).unwrap();
        git(&s, &path, &["init", "--quiet"]);
        git(&s, &path, &["config", "user.name", "Fixture"]);
        git(
            &s,
            &path,
            &["config", "user.email", "fixture@example.invalid"],
        );
        git(
            &s,
            &path,
            &[
                "-c",
                "commit.gpgSign=false",
                "commit",
                "--allow-empty",
                "-m",
                &format!("{owner} repository evidence"),
            ],
        );
        success(&index(&s, owner));
    }
    let db = rusqlite::Connection::open(s.0.join("store.db")).unwrap();
    let scopes: Vec<String> = db
        .prepare("SELECT DISTINCT scope FROM decisions ORDER BY scope")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        scopes.len(),
        2,
        "remote basename collision reused another checkout"
    );
    for owner in ["alpha", "beta"] {
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM decisions WHERE title=?1",
                [format!("{owner} repository evidence")],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        success(&index(&s, owner));
    }
    // Even a manually misbound cache directory fails before importing foreign evidence.
    for scope in scopes {
        let path = Path::new(&scope);
        git(
            &s,
            path,
            &[
                "remote",
                "set-url",
                "origin",
                "https://fixture.invalid/wrong/project.git",
            ],
        );
    }
    let before = std::fs::read(s.0.join("store.db")).unwrap();
    let out = index(&s, "alpha");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("repository cache identity mismatch"));
    assert_eq!(std::fs::read(s.0.join("store.db")).unwrap(), before);
}

#[test]
fn remote_reindex_reads_updated_decision_files() {
    let s = Sandbox::new();
    let remote = s.0.join("remotes/alpha/project.git");
    std::fs::create_dir_all(remote.join("docs")).unwrap();
    git(&s, &remote, &["init", "--quiet"]);
    git(&s, &remote, &["config", "user.name", "Fixture"]);
    git(
        &s,
        &remote,
        &["config", "user.email", "fixture@example.invalid"],
    );
    for content in [
        "Initial storage rationale",
        "Updated storage rationale",
        "New default branch rationale",
    ] {
        if content == "New default branch rationale" {
            git(&s, &remote, &["checkout", "-b", "next-main"]);
        }
        std::fs::write(remote.join("docs/decision.md"), content).unwrap();
        git(&s, &remote, &["add", "docs/decision.md"]);
        git(
            &s,
            &remote,
            &[
                "-c",
                "commit.gpgSign=false",
                "commit",
                "-m",
                "Update rationale",
            ],
        );
        success(&index(&s, "alpha"));
        let db = rusqlite::Connection::open(s.0.join("store.db")).unwrap();
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM decisions WHERE kind='adr' AND content=?1",
                [content],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 1,
            "remote reindex did not read current decision file"
        );
    }
    let db = rusqlite::Connection::open(s.0.join("store.db")).unwrap();
    let scope: String = db
        .query_row("SELECT scope FROM decisions LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let cached = Path::new(&scope).join("docs/decision.md");
    std::fs::write(&cached, "uncommitted cache edit").unwrap();
    std::fs::write(
        remote.join("docs/decision.md"),
        "Remote update conflicting with cache edit",
    )
    .unwrap();
    git(&s, &remote, &["add", "docs/decision.md"]);
    git(
        &s,
        &remote,
        &[
            "-c",
            "commit.gpgSign=false",
            "commit",
            "-m",
            "New remote rationale",
        ],
    );
    let before = std::fs::read(s.0.join("store.db")).unwrap();
    assert!(!index(&s, "alpha").status.success());
    assert_eq!(
        std::fs::read_to_string(&cached).unwrap(),
        "uncommitted cache edit"
    );
    assert_eq!(std::fs::read(s.0.join("store.db")).unwrap(), before);
    std::fs::rename(&remote, s.0.join("offline-remote")).unwrap();
    assert!(!index(&s, "alpha").status.success());
    assert_eq!(std::fs::read(s.0.join("store.db")).unwrap(), before);
}
