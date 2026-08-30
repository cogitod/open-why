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
        }
    }
}

pub fn cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cache").join("openwhy")
}

pub fn scope_for(repo: &Path) -> String {
    repo.to_string_lossy().into_owned()
}
