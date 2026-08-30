use crate::store::{cache_dir, Decision};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("git {:?} failed to run", args))?;
    if !out.status.success() {
        bail!("git {:?} exited {}", args, out.status);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn resolve_repo(repo: Option<String>) -> Result<PathBuf> {
    match repo {
        None => Ok(std::env::current_dir().context("no repo given and no current directory")?),
        Some(r) if looks_like_url(&r) => clone_repo(&r),
        Some(p) => Ok(PathBuf::from(p)),
    }
}

fn looks_like_url(r: &str) -> bool {
    r.starts_with("http://")
        || r.starts_with("https://")
        || r.starts_with("git@")
        || r.starts_with("ssh://")
}

fn clone_repo(url: &str) -> Result<PathBuf> {
    let slug = url
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or("repo");
    let dest = cache_dir().join("repos").join(slug);
    if dest.join(".git").exists() {
        let _ = Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["fetch", "--depth", "200", "origin"])
            .output();
        return Ok(dest);
    }
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let status = Command::new("git")
        .arg("clone")
        .args(["--depth", "200"])
        .arg(url)
        .arg(&dest)
        .status()
        .with_context(|| format!("failed to clone {url}"))?;
    if !status.success() {
        bail!("git clone failed for {url}");
    }
    Ok(dest)
}

pub fn mine(repo: &Path) -> Result<Vec<Decision>> {
    let mut decisions: Vec<Decision> = Vec::new();

    // 1. commit subjects + bodies
    let log = git(repo, &["log", "--all", "--format=%H%x00%an%x00%aI%x00%s%x00%b%x1e"])?;
    for entry in log.split('\u{1e}') {
        let f: Vec<&str> = entry.split('\0').collect();
        if f.len() >= 5 && f[0].trim().len() == 40 {
            decisions.push(Decision {
                sha: f[0].trim().to_string(),
                author: f[1].trim().to_string(),
                date: f[2].trim().to_string(),
                updated_at: String::new(),
                subject: f[3].trim().to_string(),
                body: f[4].trim().to_string(),
                source: "commit".to_string(),
                importance: 0.5,
                kind: "commit".to_string(),
                access_count: 0,
                effectiveness: 0.5,
                embedding: None,
            });
        }
    }

    // 2. decision files (ADRs, design docs, specs)
    let files = git(repo, &["ls-files"])?;
    for file in files.lines().map(str::trim).filter(|f| is_decision_file(f)) {
        let head = format!("HEAD:{file}");
        let Ok(content) = git(repo, &["show", head.as_str()]) else {
            continue;
        };
        let last = git(repo, &["log", "-1", "--format=%H%x00%an%x00%aI", "--", file])
            .unwrap_or_default();
        let m: Vec<&str> = last.split('\0').collect();
        let (sha, author, date) = if m.len() >= 3 {
            (
                m[0].trim().to_string(),
                m[1].trim().to_string(),
                m[2].trim().to_string(),
            )
        } else {
            (String::new(), String::new(), String::new())
        };
        let subject = file
            .rsplit('/')
            .next()
            .unwrap_or(file)
            .trim_end_matches(".md")
            .to_string();
        decisions.push(Decision {
            sha,
            author,
            date,
            updated_at: String::new(),
            subject,
            body: content.trim().to_string(),
            source: file.to_string(),
            importance: 0.5,
            kind: "adr".to_string(),
            access_count: 0,
            effectiveness: 0.5,
            embedding: None,
        });
    }

    Ok(decisions)
}

fn is_decision_file(path: &str) -> bool {
    if !path.ends_with(".md") {
        return false;
    }
    let lower = path.to_lowercase();
    lower.contains("adr")
        || lower.contains("decision")
        || lower.contains("design")
        || lower.contains("docs/")
        || lower.contains("spec")
        || lower.contains("why")
}
