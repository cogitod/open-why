//! # open-why
//!
//! A self-contained Rust core for recording why code decisions were made and
//! retrieving their source evidence, current rationale, and history.
//!
//! open-why is deliberately narrow: it is **not** a general agent-memory store. It
//! stores decision records (`decision` / `fact` / `reference` / `pattern` / `doc` /
//! `project` / `observation`) with temporal identity (`valid_from` / `valid_until` /
//! `superseded_by`; decisions are superseded, never deleted) and Git linkage
//! (commit to decision). It ranks recall with a regression-tested model:
//! reciprocal-rank fusion of a semantic arm (local MiniLM embeddings) and a lexical
//! arm (FTS5-style BM25), weighted by importance and effectiveness and decayed by
//! Ebbinghaus recency with spaced-repetition stability.
//!
//! The library and MCP server are the primary interfaces. A CLI provides convenient setup and
//! inspection over the same local SQLite store.
//!
//! ## Library
//!
//! ```no_run
//! use open_why::Store;
//! use std::path::Path;
//!
//! let store = Store::open_with_store_instance_id(
//!     Path::new("/path/to/open-why.db"),
//!     "my-app:open-why:replace-with-unique-id",
//! )?;
//! let hits = store.search("why sqlite", &["my-project"], &[], 10)?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! Mint the store identity once, keep it stable, and use `Store::open` for later lexical-only
//! opens. `Store::open_default` reads the first-binding identity from
//! `OPEN_WHY_STORE_INSTANCE_ID` and wires an embedder from the environment.
//! On Unix, store opens reject symbolic links in the database path and its directory
//! components.
//!
//! With no embedder configured (`OPEN_WHY_EMBED_MODEL_PATH` / `OPEN_WHY_EMBED_URL`),
//! search is lexical-first; with one, the semantic arm is active.

pub mod answer;
pub mod db;
pub mod embed;
pub mod mcp;
pub mod miner;
mod private_store_path;
pub mod relevance;
pub mod search;
pub mod store;

// Convenience re-exports for the library surface.
pub use db::{default_path, inspect_store, RankExplanation, Store};
pub use embed::{cosine, from_env, Embedder, HttpEmbedder, LocalEmbedder};
pub use store::{
    CommitLinkItem, CommitLinksErrorCode, CommitLinksResolution, CurrentRecordErrorCode,
    CurrentRecordResolution, Decision, EvidenceIdentity, EvidenceIdentityErrorCode,
    EvidenceIdentityResolution, ExternalDecision, GitRef, RationaleHistoryErrorCode,
    RationaleHistoryRecord, RationaleHistoryResolution, Record, RecordIdentityConflict,
    ScopedCommitLinkErrorCode, ScopedCommitLinkOutcome, ScopedCommitLinkResolution,
    ScopedCurrentEvidenceErrorCode, ScopedCurrentRecordResolution, StoreCompatibility,
    StoreCompatibilityErrorCode, StoreIdentity, StoreIdentityBindingError,
    StoreIdentityBindingErrorCode, SupersessionConflict, SupersessionCycle,
    SupersessionTargetNotFound, COMMIT_LINKS_CONTRACT, CURRENT_RATIONALE_CONTRACT,
    EVIDENCE_IDENTITY_CONTRACT, MAX_COMMIT_LINKS_PAGE_RECORDS, MAX_COMMIT_LINK_HASH_BYTES,
    MAX_COMMIT_LINK_RECORD_ID_BYTES, MAX_COMMIT_LINK_SCOPE_BYTES, MAX_COMMIT_LINK_SUBJECT_BYTES,
    MAX_HISTORY_PAGE_RECORDS, MAX_STORE_INSTANCE_ID_BYTES, MAX_SUPERSESSION_CHAIN,
    MAX_TEMPORAL_VALUE_BYTES, RATIONALE_HISTORY_CONTRACT, RATIONALE_IMPORT_CONTRACT,
    RECORD_DIGEST_CONTRACT, SCOPED_COMMIT_LINK_WRITE_CONTRACT, SCOPED_CURRENT_EVIDENCE_CONTRACT,
    STORE_SCHEMA_FAMILY, STORE_SCHEMA_VERSION,
};
