use super::common::{
    MAX_AUTHORITY_BYTES, MAX_BODY_BYTES, MAX_GIT_REFS, MAX_ID_BYTES, MAX_IMPORT_ROWS,
    MAX_QUERY_BYTES, MAX_RESULT_COUNT, MAX_TITLE_BYTES,
};
use crate::store::{
    COMMIT_LINKS_CONTRACT, CURRENT_RATIONALE_CONTRACT, MAX_COMMIT_LINKS_PAGE_RECORDS,
    MAX_COMMIT_LINK_HASH_BYTES, MAX_COMMIT_LINK_SUBJECT_BYTES, MAX_HISTORY_PAGE_RECORDS,
    MAX_TEMPORAL_VALUE_BYTES, RATIONALE_HISTORY_CONTRACT, RATIONALE_IMPORT_CONTRACT,
};
use serde_json::{json, Value};

pub(super) const MCP_CONTRACTS: &[&str] = &[
    CURRENT_RATIONALE_CONTRACT,
    RATIONALE_HISTORY_CONTRACT,
    COMMIT_LINKS_CONTRACT,
    RATIONALE_IMPORT_CONTRACT,
];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ToolKind {
    Ask,
    Index,
    Capture,
    Import,
    Search,
    Get,
    History,
    CommitLinks,
    Link,
    Feedback,
}

#[derive(Clone, Copy)]
pub(super) struct ToolSpec {
    pub(super) name: &'static str,
    description: &'static str,
    pub(super) kind: ToolKind,
}

pub(super) const TOOL_SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "open-why_ask",
        description: "Ask why a decision was made in an explicitly identified repository.",
        kind: ToolKind::Ask,
    },
    ToolSpec {
        name: "open-why_index",
        description: "Index an explicitly identified repository's decision history.",
        kind: ToolKind::Index,
    },
    ToolSpec {
        name: "open-why_capture",
        description: "Capture a bounded decision in an explicit scope.",
        kind: ToolKind::Capture,
    },
    ToolSpec {
        name: "open-why_import",
        description: "Import bounded records into one explicit scope.",
        kind: ToolKind::Import,
    },
    ToolSpec {
        name: "open-why_search",
        description: "Return bounded stable-ID previews from an explicit scope.",
        kind: ToolKind::Search,
    },
    ToolSpec {
        name: "open-why_get",
        description: "Resolve an exact stable ID to its complete current rationale and evidence.",
        kind: ToolKind::Get,
    },
    ToolSpec {
        name: "open-why_history",
        description: "Page the exact supersession chain forward from a given ID, with complete records and Git evidence. Pass the earliest (originating) ID to see the full chain; a current ID returns only itself.",
        kind: ToolKind::History,
    },
    ToolSpec {
        name: "open-why_commit_links",
        description: "Page exact direct rationale links for one Git commit hash and scope.",
        kind: ToolKind::CommitLinks,
    },
    ToolSpec {
        name: "open-why_link",
        description: "Link a Git commit to a decision in an explicit scope.",
        kind: ToolKind::Link,
    },
    ToolSpec {
        name: "open-why_feedback",
        description: "Record retrieval feedback for a decision in an explicit scope.",
        kind: ToolKind::Feedback,
    },
];
fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

pub(super) fn import_row_schema() -> Value {
    object_schema(
        json!({
            "id":{"type":"string","maxLength":MAX_ID_BYTES},
            "kind":{"type":"string","maxLength":128},
            "title":{"type":"string","maxLength":MAX_TITLE_BYTES},
            "content":{"type":"string","maxLength":MAX_BODY_BYTES},
            "importance":{"type":"number","minimum":0,"maximum":1},
            "source":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
            "author":{"type":"string","maxLength":MAX_TITLE_BYTES},
            "date":{"type":"string","maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "updated_at":{"type":["string","null"],"maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "accessed_count":{"type":["integer","null"]},
            "times_injected":{"type":["integer","null"]},
            "effectiveness":{"type":["number","null"],"minimum":0,"maximum":1},
            "tags":{"type":["string","null"],"maxLength":MAX_BODY_BYTES},
            "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
            "valid_from":{"type":["string","null"],"maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "valid_until":{"type":["string","null"],"maxLength":MAX_TEMPORAL_VALUE_BYTES},
            "superseded_by":{"type":["string","null"],"maxLength":MAX_ID_BYTES},
            "fact_key":{"type":["string","null"],"maxLength":MAX_ID_BYTES},
            "git_refs":{"type":"array","maxItems":MAX_GIT_REFS,"items":object_schema(
                json!({
                    "commit_hash":{"type":"string","maxLength":MAX_COMMIT_LINK_HASH_BYTES},
                    "commit_subject":{"type":"string","maxLength":MAX_COMMIT_LINK_SUBJECT_BYTES}
                }),
                &["commit_hash", "commit_subject"]
            )}
        }),
        &["id", "kind", "title", "content", "scope"],
    )
}

pub(super) fn input_schema(kind: ToolKind) -> Value {
    match kind {
        ToolKind::Ask => object_schema(
            json!({
                "question": {"type":"string","maxLength":MAX_QUERY_BYTES},
                "repo": {"type":"string","maxLength":MAX_AUTHORITY_BYTES,"description":"Absolute repository path"}
            }),
            &["question", "repo"],
        ),
        ToolKind::Index => object_schema(
            json!({"repo":{"type":"string","maxLength":MAX_AUTHORITY_BYTES,"description":"Absolute repository path"}}),
            &["repo"],
        ),
        ToolKind::Capture => object_schema(
            json!({
                "kind":{"type":"string","maxLength":128},
                "title":{"type":"string","maxLength":MAX_TITLE_BYTES},
                "content":{"type":"string","maxLength":MAX_BODY_BYTES},
                "importance":{"type":"number","minimum":0,"maximum":1},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "valid_from":{"type":"string","maxLength":MAX_TEMPORAL_VALUE_BYTES},
                "fact_key":{"type":"string","maxLength":MAX_ID_BYTES},
                "supersedes":{"type":"string","maxLength":MAX_ID_BYTES}
            }),
            &["title", "content", "scope"],
        ),
        ToolKind::Import => object_schema(
            json!({
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "rows":{"type":"array","maxItems":MAX_IMPORT_ROWS,"items":import_row_schema()}
            }),
            &["scope", "rows"],
        ),
        ToolKind::Search => object_schema(
            json!({
                "query":{"type":"string","maxLength":MAX_QUERY_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_RESULT_COUNT},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "type":{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"string"}}]},
                "historical":{"type":"boolean"}
            }),
            &["query", "scope"],
        ),
        ToolKind::Get => object_schema(
            json!({
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES}
            }),
            &["id", "scope"],
        ),
        ToolKind::History => object_schema(
            json!({
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_HISTORY_PAGE_RECORDS},
                "cursor":{"type":"string","maxLength":MAX_ID_BYTES}
            }),
            &["id", "scope"],
        ),
        ToolKind::CommitLinks => object_schema(
            json!({
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES},
                "commit":{"type":"string","maxLength":MAX_COMMIT_LINK_HASH_BYTES},
                "limit":{"type":"integer","minimum":1,"maximum":MAX_COMMIT_LINKS_PAGE_RECORDS},
                "cursor":{"type":"string","maxLength":MAX_ID_BYTES}
            }),
            &["scope", "commit"],
        ),
        ToolKind::Link => object_schema(
            json!({
                "commit":{"type":"string","maxLength":MAX_COMMIT_LINK_HASH_BYTES},
                "decision":{"type":"string","maxLength":MAX_ID_BYTES},
                "subject":{"type":"string","maxLength":MAX_COMMIT_LINK_SUBJECT_BYTES},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES}
            }),
            &["commit", "decision", "scope"],
        ),
        ToolKind::Feedback => object_schema(
            json!({
                "id":{"type":"string","maxLength":MAX_ID_BYTES},
                "helpful":{"type":"boolean"},
                "scope":{"type":"string","maxLength":MAX_AUTHORITY_BYTES}
            }),
            &["id", "helpful", "scope"],
        ),
    }
}

pub(super) fn registry_tools() -> Vec<Value> {
    TOOL_SPECS
        .iter()
        .map(|spec| {
            let mut tool = json!({
                "name": spec.name,
                "description": spec.description,
                "inputSchema": input_schema(spec.kind)
            });
            if spec.kind == ToolKind::Import {
                tool["_meta"] = json!({"contract":RATIONALE_IMPORT_CONTRACT});
            }
            tool
        })
        .collect()
}

pub(super) fn registry_digest() -> String {
    let bytes = serde_json::to_vec(&registry_tools()).expect("tool registry serializes");
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
