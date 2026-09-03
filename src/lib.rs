//! # open-why
//!
//! The thin "why" layer for AI agents — a self-contained Rust core that remembers
//! *why* decisions were made, with the evidence attached, and recalls them by hybrid
//! rank (semantic + lexical + recency + importance).
//!
//! open-why is deliberately narrow: it is **not** a general agent-memory store. It
//! stores decision records (`decision` / `fact` / `reference` / `pattern` / `doc` /
//! `project` / `observation`) with temporal identity (`valid_from` / `valid_until` /
//! `superseded_by` — decisions are superseded, never deleted) and git linkage
//! (commit ↔ decision), and it ranks recall the way cogitod's calibrated engine does:
//! reciprocal-rank fusion of a semantic arm (local MiniLM embeddings) and a lexical
//! arm (FTS5-style BM25), weighted by importance and effectiveness and decayed by
//! Ebbinghaus recency with spaced-repetition stability.
//!
//! It ships as a library, a CLI, and an MCP server, all over one local SQLite store.
//!
//! ## Library
//!
//! ```no_run
//! use open_why::Store;
//!
//! let store = Store::open_default()?;                 // wires an embedder from the env
//! let hits = store.search("why sqlite", &["my-project"], &[], 10)?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! With no embedder configured (`OPEN_WHY_EMBED_MODEL_PATH` / `OPEN_WHY_EMBED_URL`),
//! search is lexical-first; with one, the semantic arm is active.

pub mod answer;
pub mod db;
pub mod embed;
pub mod mcp;
pub mod miner;
pub mod relevance;
pub mod search;
pub mod store;

// Convenience re-exports — the library surface.
pub use db::{default_path, RankExplanation, Store};
pub use embed::{cosine, from_env, Embedder, HttpEmbedder, LocalEmbedder};
pub use store::{Decision, EvidenceRecord, ExternalDecision, GitRef, Record};
