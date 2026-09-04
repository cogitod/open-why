use crate::embed::Embedder;
use crate::store::{
    CommitLinkItem, CommitLinksErrorCode, CommitLinksResolution, CurrentRecordErrorCode,
    CurrentRecordResolution, Decision, EvidenceIdentity, EvidenceIdentityErrorCode,
    EvidenceIdentityResolution, ExternalDecision, GitRef, RationaleHistoryErrorCode,
    RationaleHistoryRecord, RationaleHistoryResolution, Record, RecordIdentityConflict,
    ScopedCommitLinkErrorCode, ScopedCommitLinkOutcome, ScopedCommitLinkResolution,
    ScopedCurrentEvidenceErrorCode, ScopedCurrentRecordResolution, StoreCompatibility,
    StoreCompatibilityErrorCode, StoreIdentity, StoreIdentityBindingError,
    StoreIdentityBindingErrorCode, SupersessionConflict, SupersessionCycle,
    SupersessionTargetNotFound, COMMIT_LINKS_CONTRACT, CURRENT_RATIONALE_CONTRACT,
    EVIDENCE_IDENTITY_CONTRACT, MAX_COMMIT_LINKS_PAGE_RECORDS, MAX_COMMIT_LINKS_PAGE_SOURCE_BYTES,
    MAX_COMMIT_LINK_HASH_BYTES, MAX_COMMIT_LINK_RECORD_ID_BYTES, MAX_COMMIT_LINK_SCOPE_BYTES,
    MAX_COMMIT_LINK_SUBJECT_BYTES, MAX_HISTORY_PAGE_GIT_REFS, MAX_HISTORY_PAGE_RECORDS,
    MAX_HISTORY_PAGE_SOURCE_BYTES, MAX_STORE_INSTANCE_ID_BYTES, MAX_SUPERSESSION_CHAIN,
    MAX_TEMPORAL_VALUE_BYTES, RATIONALE_HISTORY_CONTRACT, RECORD_DIGEST_CONTRACT,
    SCOPED_COMMIT_LINK_WRITE_CONTRACT, SCOPED_CURRENT_EVIDENCE_CONTRACT, STORE_SCHEMA_FAMILY,
    STORE_SCHEMA_VERSION,
};
use anyhow::{Context, Result};
use rusqlite::{
    params, Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior,
};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};

mod capture_store;
mod compatibility;
mod digest;
mod evidence;
mod lifecycle;
mod query;
mod ranking;
mod records;
mod schema;
mod time;

use compatibility::*;
use digest::*;
use ranking::{rank, rank_by, RankRow};
use schema::*;
use time::*;

pub fn default_path() -> PathBuf {
    std::env::var("OPEN_WHY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::store::cache_dir().join("open-why.db"))
}

/// The "why" core store. Owns the decision record (temporal + provenance + git linkage).
pub struct Store {
    conn: Connection,
    embedder: Option<Box<dyn Embedder>>,
    _store_parent: Option<std::fs::File>,
}

struct HistoryNode {
    id: String,
    scope: String,
    superseded_by: Option<String>,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

struct CurrentNode {
    id: String,
    superseded_by: Option<String>,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

struct CurrentEvidenceRead {
    resolution: CurrentRecordResolution,
    identity: Option<EvidenceIdentity>,
}

#[derive(Clone)]
struct RecordDigestRow {
    id: String,
    scope: String,
    kind: String,
    title: String,
    content: String,
    importance: f64,
    source: String,
    author: String,
    commit_sha: String,
    date: String,
    tags: Option<String>,
    fact_key: Option<String>,
    valid_from: Option<String>,
    declared_valid_until: Option<String>,
    sealed_digest: Option<String>,
}

struct HistoryPageRequest<'a> {
    id: &'a str,
    scope: &'a str,
    page_cursor: Option<&'a str>,
    limit: usize,
    as_of: i64,
    chain_cap: usize,
}

struct ExternalCaptureRequest<'a> {
    decision: &'a Decision,
    scope: &'a str,
    id: &'a str,
    valid_from: Option<&'a str>,
    fact_key: Option<&'a str>,
    supersedes: Option<&'a str>,
}

/// Inspect an existing store without creating or migrating any filesystem or
/// database state.
pub fn inspect_store(path: &Path) -> Result<StoreCompatibility> {
    compatibility::inspect_store(path)
}

impl Store {
    /// Open a store without an embedder (lexical-first). Kept as the explicit no-embedder entry
    /// point; every command path uses `open_default` so the semantic arm is active uniformly.
    #[allow(dead_code)]
    pub fn open(path: &Path) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, None, None)
    }

    /// Open or migrate a store with an explicit provider-owned identity. The
    /// identity is required for first binding and must match on later bound opens.
    pub fn open_with_store_instance_id(path: &Path, store_instance_id: &str) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, None, Some(store_instance_id))
    }

    /// Open the default store, wiring an embedder from the environment when one is configured
    /// (`OPEN_WHY_EMBED_MODEL_PATH` or `OPEN_WHY_EMBED_URL`). This is the entry point every CLI
    /// command and the MCP server use, so the semantic arm is active uniformly.
    pub fn open_default() -> Result<Store> {
        let store_instance_id = std::env::var("OPEN_WHY_STORE_INSTANCE_ID").ok();
        Self::open_with_embedder_and_identity(
            &default_path(),
            crate::embed::from_env()?,
            store_instance_id.as_deref(),
        )
    }

    pub fn open_with_embedder(path: &Path, embedder: Option<Box<dyn Embedder>>) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, embedder, None)
    }

    pub fn open_with_embedder_and_store_instance_id(
        path: &Path,
        embedder: Option<Box<dyn Embedder>>,
        store_instance_id: &str,
    ) -> Result<Store> {
        Self::open_with_embedder_and_identity(path, embedder, Some(store_instance_id))
    }

    /// Best-effort embedding of the searchable text: `title\ncontent`, then the
    /// space-joined tag array when present. Returns the JSON vector
    /// when an embedder is configured and succeeds; `None` keeps the row lexical.
    fn embed_text(&self, title: &str, content: &str, tags: Option<&str>) -> Option<String> {
        let embedder = self.embedder.as_ref()?;
        let mut text = String::new();
        let t = title.trim();
        if !t.is_empty() {
            text.push_str(t);
            text.push('\n');
        }
        text.push_str(content);
        if let Some(raw) = tags {
            if let Ok(v) = serde_json::from_str::<Vec<String>>(raw) {
                if !v.is_empty() {
                    text.push('\n');
                    text.push_str(&v.join(" "));
                }
            }
        }
        let vec = embedder.embed(&text).ok()?;
        serde_json::to_string(&vec).ok()
    }

    fn query_embedding(&self, query: &str) -> Option<Vec<f32>> {
        self.embedder.as_ref()?.embed(query).ok()
    }
}

/// Per-result ranking explanation for why a row ranked where it did. Exposed by `--explain`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RankExplanation {
    pub similarity: f64,
    pub importance: f64,
    pub effectiveness: f64,
    pub age_days: f64,
    pub recency_decay: f64,
    pub hybrid_score: f64,
    pub semantic_rank: Option<usize>,
    pub lexical_rank: Option<usize>,
    pub rrf_score: f64,
}

/// A search result set paired with its ranking explanation, as returned by the `--explain`
/// and `--explain-drops` paths.
pub type Explained = Vec<(Record, RankExplanation)>;

pub(crate) fn store_error_is_retryable(error: &anyhow::Error) -> bool {
    digest::store_error_is_retryable(error)
}

pub(crate) fn epoch_to_iso(secs: i64) -> String {
    time::epoch_to_iso(secs)
}

#[cfg(test)]
mod tests;
