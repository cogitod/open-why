use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub sha: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub body: String,
    pub source: String,
    pub importance: f64,
    pub kind: String,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
}

impl Default for Decision {
    fn default() -> Self {
        Decision {
            sha: String::new(),
            author: String::new(),
            date: String::new(),
            subject: String::new(),
            body: String::new(),
            source: String::new(),
            importance: 0.5,
            kind: String::new(),
            embedding: None,
        }
    }
}

pub fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cache").join("open-why")
}

pub fn scope_for(repo: &Path) -> String {
    repo.to_string_lossy().into_owned()
}

fn default_importance() -> f64 {
    0.5
}

fn default_scope() -> String {
    "global".to_string()
}

/// A git commit bound to a decision (the "why" for that commit).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitRef {
    pub commit_hash: String,
    pub commit_subject: String,
}

/// A full decision record as returned to a client, including its id and temporal
/// window. Mirrors the `decisions` table so a consumer (cogitod) can round-trip a
/// record exactly, not just its text.
#[derive(Debug, Clone, Serialize)]
pub struct Record {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub content: String,
    pub importance: f64,
    pub source: String,
    pub author: String,
    pub date: String,
    pub commit_sha: String,
    pub scope: String,
    pub superseded_by: Option<String>,
    pub valid_from: Option<String>,
    pub valid_until: Option<String>,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
}

/// A decision imported with an externally-minted id (e.g. a cogitod memory UUID).
/// Carries the full temporal window (`valid_from`/`valid_until`), supersession, and
/// git linkage so an importer can reproduce a decision exactly, not just its text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalDecision {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub content: String,
    #[serde(default = "default_importance")]
    pub importance: f64,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub date: String,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default)]
    pub valid_from: Option<String>,
    #[serde(default)]
    pub valid_until: Option<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub fact_key: Option<String>,
    #[serde(default)]
    pub git_refs: Vec<GitRef>,
}
