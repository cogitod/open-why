use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CURRENT_RATIONALE_CONTRACT: &str = "open-why.current-rationale/v1";
pub const RATIONALE_HISTORY_CONTRACT: &str = "open-why.rationale-history/v1";
pub const COMMIT_LINKS_CONTRACT: &str = "open-why.commit-links/v1";
pub const RATIONALE_IMPORT_CONTRACT: &str = "open-why.rationale-import/v1";
pub const EVIDENCE_IDENTITY_CONTRACT: &str = "open-why.evidence-identity/v1";
pub const SCOPED_CURRENT_EVIDENCE_CONTRACT: &str = "open-why.scoped-current-evidence/v1";
pub const RECORD_DIGEST_CONTRACT: &str = "open-why.record-digest/v1";
pub const STORE_SCHEMA_FAMILY: &str = "open-why";
pub const STORE_SCHEMA_VERSION: u32 = 1;
pub const MAX_SUPERSESSION_CHAIN: usize = 64;
pub const MAX_HISTORY_PAGE_RECORDS: usize = 3;
pub const MAX_COMMIT_LINKS_PAGE_RECORDS: usize = 20;
pub(crate) const MAX_HISTORY_PAGE_SOURCE_BYTES: usize = 3 * 1024 * 1024;
pub(crate) const MAX_HISTORY_PAGE_GIT_REFS: usize = 300;
pub(crate) const MAX_COMMIT_LINK_SUBJECT_BYTES: usize = 4 * 1024;
pub(crate) const MAX_COMMIT_LINK_RECORD_ID_BYTES: usize = 512;
pub(crate) const MAX_COMMIT_LINKS_PAGE_SOURCE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub sha: String,
    pub author: String,
    pub date: String,
    #[serde(skip)]
    pub updated_at: String,
    pub subject: String,
    pub body: String,
    pub source: String,
    pub importance: f64,
    pub kind: String,
    #[serde(skip)]
    pub access_count: i64,
    #[serde(skip)]
    pub effectiveness: f64,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
}

impl Default for Decision {
    fn default() -> Self {
        Decision {
            sha: String::new(),
            author: String::new(),
            date: String::new(),
            updated_at: String::new(),
            subject: String::new(),
            body: String::new(),
            source: String::new(),
            importance: 0.5,
            kind: String::new(),
            access_count: 0,
            effectiveness: 0.5,
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

/// Durable identity for one physical open-why store and its verified schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoreIdentity {
    pub store_instance_id: String,
    pub schema_family: &'static str,
    pub schema_version: u32,
    pub schema_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreCompatibilityErrorCode {
    SchemaNewer,
    PartialMigration,
    ChecksumMismatch,
    ShapeDrift,
    SchemaCorrupt,
    LiveWalIndeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreIdentityBindingErrorCode {
    IdentityRequired,
    InvalidIdentity,
    IdentityMismatch,
}

/// Typed failure while binding a provider-owned identity to a store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreIdentityBindingError {
    pub code: StoreIdentityBindingErrorCode,
    pub message: &'static str,
}

impl std::fmt::Display for StoreIdentityBindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self.code {
            StoreIdentityBindingErrorCode::IdentityRequired => "identity_required",
            StoreIdentityBindingErrorCode::InvalidIdentity => "invalid_identity",
            StoreIdentityBindingErrorCode::IdentityMismatch => "identity_mismatch",
        };
        write!(f, "{code}: {}", self.message)
    }
}

impl std::error::Error for StoreIdentityBindingError {}

/// Read-only compatibility result for a database path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StoreCompatibility {
    Missing,
    Uninitialized,
    MigrationRequired {
        from: u32,
        to: u32,
        plan_digest: String,
    },
    Compatible {
        identity: StoreIdentity,
    },
    Incompatible {
        code: StoreCompatibilityErrorCode,
        message: String,
        found_version: Option<u32>,
    },
}

/// Collision-resistant identity for one immutable rationale envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvidenceIdentity {
    pub contract: &'static str,
    pub record_digest_contract: &'static str,
    pub store_instance_id: String,
    pub scope: String,
    pub record_id: String,
    pub record_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceIdentityErrorCode {
    NotFound,
    IdentityConflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum EvidenceIdentityResolution {
    Ok {
        identity: EvidenceIdentity,
    },
    Error {
        contract: &'static str,
        code: EvidenceIdentityErrorCode,
        message: String,
        retryable: bool,
    },
}

/// Typed error returned when a write tries to change a sealed record envelope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordIdentityConflict;

impl std::fmt::Display for RecordIdentityConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("identity_conflict: immutable record evidence does not match its sealed digest")
    }
}

impl std::error::Error for RecordIdentityConflict {}

/// Typed error returned when a predecessor already names another successor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupersessionConflict;

impl std::fmt::Display for SupersessionConflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("supersession_conflict: predecessor already names a different successor")
    }
}

impl std::error::Error for SupersessionConflict {}

/// Typed error returned when a requested predecessor does not exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupersessionTargetNotFound;

impl std::fmt::Display for SupersessionTargetNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("supersession_target_not_found: predecessor was not found")
    }
}

impl std::error::Error for SupersessionTargetNotFound {}

/// Typed error returned when a requested predecessor relation would create a cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupersessionCycle;

impl std::fmt::Display for SupersessionCycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("supersession_cycle: requested relation would create a cycle")
    }
}

impl std::error::Error for SupersessionCycle {}

/// Shared UTF-8 byte limit for canonical temporal values.
pub const MAX_TEMPORAL_VALUE_BYTES: usize = 128;

/// One evidence-bound read of the current record reached from a stable record ID.
///
/// `requested_id` may name a superseded record. `record` is always the current,
/// active end of that supersession chain; `supersession_chain` makes the resolution
/// explicit so consumers never present retired rationale as current. Git references
/// belong to the returned current record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CurrentRecordErrorCode {
    NotFound,
    NotYetValid,
    ExpiredWithoutSuccessor,
    BrokenChain,
    Cycle,
    TraversalLimit,
    InvalidTemporalData,
}

impl std::fmt::Display for CurrentRecordErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let code = match self {
            Self::NotFound => "not_found",
            Self::NotYetValid => "not_yet_valid",
            Self::ExpiredWithoutSuccessor => "expired_without_successor",
            Self::BrokenChain => "broken_chain",
            Self::Cycle => "cycle",
            Self::TraversalLimit => "traversal_limit",
            Self::InvalidTemporalData => "invalid_temporal_data",
        };
        f.write_str(code)
    }
}

impl std::error::Error for CurrentRecordErrorCode {}

/// Exact-ID resolution is an explicit contract rather than `Option<Record>` so a
/// caller can distinguish absence from damaged or no-longer-current history.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CurrentRecordResolution {
    Ok {
        contract: &'static str,
        as_of: String,
        requested_id: String,
        current_id: String,
        record: Box<Record>,
        git_refs: Vec<GitRef>,
        supersession_chain: Vec<String>,
    },
    Error {
        contract: &'static str,
        as_of: String,
        requested_id: String,
        code: CurrentRecordErrorCode,
        message: String,
        retryable: bool,
    },
}

/// Scoped exact-current read with the current record's verified identity.
///
/// This is a library contract. The existing MCP result remains unchanged.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ScopedCurrentRecordResolution {
    Ok {
        contract: &'static str,
        as_of: String,
        requested_id: String,
        current_id: String,
        record: Box<Record>,
        git_refs: Vec<GitRef>,
        supersession_chain: Vec<String>,
        evidence_identity: EvidenceIdentity,
    },
    Error {
        contract: &'static str,
        as_of: String,
        requested_id: String,
        code: ScopedCurrentEvidenceErrorCode,
        message: String,
        retryable: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopedCurrentEvidenceErrorCode {
    NotFound,
    NotYetValid,
    ExpiredWithoutSuccessor,
    BrokenChain,
    Cycle,
    TraversalLimit,
    InvalidTemporalData,
    IdentityConflict,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RationaleHistoryErrorCode {
    NotFound,
    BrokenChain,
    Cycle,
    TraversalLimit,
    InvalidTemporalData,
    InvalidCursor,
    ResponseTooLarge,
}

/// One complete historical record and the Git evidence bound to that exact
/// point in the supersession chain.
#[derive(Debug, Clone, Serialize)]
pub struct RationaleHistoryRecord {
    pub record: Box<Record>,
    pub git_refs: Vec<GitRef>,
}

/// A stable-ID page over one exact forward supersession chain.
///
/// `cursor`, when present, names the first record in the returned page. It is
/// valid only when it occurs on the chain rooted at `requested_id`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RationaleHistoryResolution {
    Ok {
        contract: &'static str,
        as_of: String,
        requested_id: String,
        page_start_id: String,
        records: Vec<RationaleHistoryRecord>,
        next_cursor: Option<String>,
        complete: bool,
    },
    Error {
        contract: &'static str,
        as_of: String,
        requested_id: String,
        code: RationaleHistoryErrorCode,
        message: String,
        retryable: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitLinksErrorCode {
    NotFound,
    InvalidCursor,
    ResponseTooLarge,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommitLinkItem {
    pub record_id: String,
    pub commit_subject: String,
}

/// A bounded exact-hash lookup of direct rationale links.
///
/// Record IDs remain historical identities; callers compose with
/// `get_current_evidence` when they need the current rationale.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CommitLinksResolution {
    Ok {
        contract: &'static str,
        scope: String,
        commit: String,
        items: Vec<CommitLinkItem>,
        next_cursor: Option<String>,
    },
    Error {
        contract: &'static str,
        code: CommitLinksErrorCode,
        message: String,
        retryable: bool,
    },
}

/// A full decision record as returned to a client, including its id and temporal
/// window. Mirrors the `decisions` table so a consumer can round-trip a record
/// exactly, not just its text.
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
    pub updated_at: String,
    pub access_count: i64,
    pub effectiveness: f64,
    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
}

/// A decision imported with an externally minted stable ID.
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
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub accessed_count: Option<i64>,
    #[serde(default)]
    pub times_injected: Option<i64>,
    #[serde(default)]
    pub effectiveness: Option<f64>,
    #[serde(default)]
    pub tags: Option<String>,
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
