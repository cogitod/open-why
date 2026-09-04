use super::args::{
    explicit_repo, explicit_scope, kinds_from, optional_string, required_exact_non_blank_string,
    required_string, validate_import_row, validate_import_shape,
};
use super::catalog::{input_schema, ToolKind, TOOL_SPECS};
use super::common::{
    tool_wire_size, ToolError, ToolResult, MAX_AUTHORITY_BYTES, MAX_BODY_BYTES, MAX_ID_BYTES,
    MAX_IMPORT_BYTES, MAX_IMPORT_ROWS, MAX_PREVIEW_BYTES, MAX_QUERY_BYTES, MAX_RESPONSE_BYTES,
    MAX_RESULT_COUNT, MAX_TITLE_BYTES,
};
use crate::store::{
    self, CommitLinksErrorCode, CommitLinksResolution, CurrentRecordErrorCode,
    CurrentRecordResolution, EvidenceIdentityResolution, ExternalDecision,
    RationaleHistoryErrorCode, RationaleHistoryResolution, Record, RecordIdentityConflict,
    ScopedCommitLinkErrorCode, ScopedCommitLinkResolution, COMMIT_LINKS_CONTRACT,
    MAX_COMMIT_LINKS_PAGE_RECORDS, MAX_COMMIT_LINK_HASH_BYTES, MAX_COMMIT_LINK_SUBJECT_BYTES,
    MAX_HISTORY_PAGE_RECORDS, MAX_SUPERSESSION_CHAIN, MAX_TEMPORAL_VALUE_BYTES,
    RATIONALE_HISTORY_CONTRACT, RATIONALE_IMPORT_CONTRACT,
};
use crate::{db, miner};
use serde_json::{json, Value};

pub(super) fn dispatch_tool(store: &db::Store, name: &str, args: &Value, as_of: i64) -> ToolResult {
    let Some(spec) = TOOL_SPECS.iter().find(|spec| spec.name == name) else {
        return Err(ToolError::new(
            "unknown_tool",
            format!("unknown tool `{name}`"),
        ));
    };
    let schema = input_schema(spec.kind);
    let allowed = schema["properties"]
        .as_object()
        .expect("registry properties are objects");
    for key in args
        .as_object()
        .expect("dispatch receives object arguments")
        .keys()
    {
        if !allowed.contains_key(key) {
            return Err(ToolError::new(
                "invalid_arguments",
                format!("unknown argument `{key}` for tool `{name}`"),
            ));
        }
    }
    match spec.kind {
        ToolKind::Ask => ask_tool(store, args),
        ToolKind::Index => index_tool(store, args),
        ToolKind::Capture => capture_tool(store, args),
        ToolKind::Import => import_tool(store, args),
        ToolKind::Search => search_tool(store, args),
        ToolKind::Get => with_as_of(get_tool(store, args, as_of), as_of),
        ToolKind::History => with_as_of(history_tool(store, args, as_of), as_of),
        ToolKind::CommitLinks => commit_links_tool(store, args),
        ToolKind::Link => link_tool(store, args),
        ToolKind::Feedback => feedback_tool(store, args),
    }
}

fn with_as_of(mut result: ToolResult, as_of: i64) -> ToolResult {
    if let Err(error) = &mut result {
        if let Some(object) = error.payload.as_object_mut() {
            object
                .entry("as_of")
                .or_insert_with(|| Value::String(db::epoch_to_iso(as_of)));
        }
    }
    result
}

fn ask_tool(store: &db::Store, args: &Value) -> ToolResult {
    let question = required_string(args, "question", MAX_QUERY_BYTES)?;
    let repo = explicit_repo(args)?;
    let repo = miner::resolve_repo(Some(repo.to_owned())).map_err(ToolError::internal)?;
    let scope = store::scope_for(&repo);
    if store.count_for_scope(&scope).map_err(ToolError::internal)? == 0 {
        let decisions = miner::mine(&repo).map_err(ToolError::internal)?;
        store
            .import_decisions(&scope, &decisions)
            .map_err(ToolError::internal)?;
    }
    let records = store
        .search_records(question, &[scope.as_str()], &[], 5)
        .map_err(ToolError::internal)?;
    Ok(json!({"status":"ok","scope":scope,"results":previews(records)}))
}

fn index_tool(store: &db::Store, args: &Value) -> ToolResult {
    let repo = explicit_repo(args)?;
    let repo = miner::resolve_repo(Some(repo.to_owned())).map_err(ToolError::internal)?;
    let scope = store::scope_for(&repo);
    let decisions = miner::mine(&repo).map_err(ToolError::internal)?;
    let count = decisions.len();
    store
        .import_decisions(&scope, &decisions)
        .map_err(ToolError::internal)?;
    Ok(json!({"status":"ok","scope":scope,"indexed":count}))
}

fn capture_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let title = required_string(args, "title", MAX_TITLE_BYTES)?;
    let content = required_string(args, "content", MAX_BODY_BYTES)?;
    let kind = optional_string(args, "kind", 128)?.unwrap_or("decision");
    let importance = match args.get("importance") {
        None => 0.5,
        Some(value) => value
            .as_f64()
            .filter(|value| (0.0..=1.0).contains(value))
            .ok_or_else(|| {
                ToolError::new(
                    "invalid_arguments",
                    "`importance` must be a finite number from 0 to 1",
                )
            })?,
    };
    let id = optional_string(args, "id", MAX_ID_BYTES)?;
    let valid_from = optional_string(args, "valid_from", MAX_TEMPORAL_VALUE_BYTES)?;
    let fact_key = optional_string(args, "fact_key", MAX_ID_BYTES)?;
    let supersedes = optional_string(args, "supersedes", MAX_ID_BYTES)?;
    if let Some(valid_from) = valid_from {
        if !store
            .temporal_value_is_valid(valid_from)
            .map_err(ToolError::internal)?
        {
            return Err(ToolError::new(
                "invalid_arguments",
                "`valid_from` must be a valid ISO timestamp",
            ));
        }
    }
    if let Some(supersedes) = supersedes.filter(|value| !value.is_empty()) {
        if !store
            .record_belongs_to_scope(supersedes, scope)
            .map_err(ToolError::internal)?
        {
            return Err(ToolError::new(
                "scope_mismatch",
                format!("superseded record `{supersedes}` is not in scope `{scope}`"),
            ));
        }
    }
    let decision = store::Decision {
        subject: title.to_owned(),
        body: content.to_owned(),
        kind: kind.to_owned(),
        importance,
        source: "capture".to_owned(),
        ..store::Decision::default()
    };
    let id = match id.filter(|value| !value.is_empty()) {
        Some(id) => store.capture_external(&decision, scope, id, valid_from, fact_key, supersedes),
        None => store.capture(&decision, scope, supersedes),
    }
    .map_err(|error| {
        if error.downcast_ref::<CurrentRecordErrorCode>()
            == Some(&CurrentRecordErrorCode::InvalidTemporalData)
        {
            ToolError::new(
                "invalid_arguments",
                "supersession predecessor has invalid temporal data",
            )
        } else {
            ToolError::internal(error)
        }
    })?;
    Ok(json!({"status":"ok","id":id,"scope":scope}))
}

fn import_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let rows_value = args
        .get("rows")
        .ok_or_else(|| ToolError::new("invalid_arguments", "`rows` is required"))?;
    let aggregate = serde_json::to_vec(args).map_err(ToolError::internal)?;
    if aggregate.len() > MAX_IMPORT_BYTES {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("aggregate import exceeds {MAX_IMPORT_BYTES} UTF-8 bytes"),
        ));
    }
    validate_import_shape(rows_value)?;
    let rows: Vec<ExternalDecision> = serde_json::from_value(rows_value.clone())
        .map_err(|error| ToolError::new("invalid_arguments", format!("invalid rows: {error}")))?;
    if rows.len() > MAX_IMPORT_ROWS {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("import exceeds {MAX_IMPORT_ROWS} records"),
        ));
    }
    for row in &rows {
        validate_import_row(store, row, scope)?;
    }
    let imported = store.import_external(&rows).map_err(|error| {
        if error.downcast_ref::<RecordIdentityConflict>().is_some() {
            ToolError::resolution(json!({
                "contract":RATIONALE_IMPORT_CONTRACT,
                "status":"error",
                "code":"identity_conflict",
                "message":"record identity conflicts with sealed evidence",
                "retryable":false
            }))
        } else {
            ToolError::internal(error)
        }
    })?;
    Ok(json!({
        "contract":RATIONALE_IMPORT_CONTRACT,
        "status":"ok",
        "scope":scope,
        "imported":imported
    }))
}

fn search_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let query = required_string(args, "query", MAX_QUERY_BYTES)?;
    let limit = match args.get("limit") {
        None => 10,
        Some(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .ok_or_else(|| {
                ToolError::new("invalid_arguments", "`limit` must be a positive integer")
            })?,
    };
    if !(1..=MAX_RESULT_COUNT).contains(&limit) {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("`limit` must be from 1 to {MAX_RESULT_COUNT}"),
        ));
    }
    let historical = match args.get("historical") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| ToolError::new("invalid_arguments", "`historical` must be a boolean"))?,
    };
    let kinds = kinds_from(args)?;
    let records = store
        .search_records_with(query, &[scope], &kinds, limit, historical)
        .map_err(ToolError::internal)?;
    Ok(json!({"status":"ok","scope":scope,"results":previews(records)}))
}

fn bounded_preview(content: &str) -> (&str, bool) {
    if content.len() <= MAX_PREVIEW_BYTES {
        return (content, false);
    }
    let mut end = MAX_PREVIEW_BYTES;
    while end > 0 && !content.is_char_boundary(end) {
        end -= 1;
    }
    (&content[..end], true)
}

pub(super) fn previews(records: Vec<Record>) -> Vec<Value> {
    records
        .into_iter()
        .map(|record| {
            let (preview, preview_truncated) = bounded_preview(&record.content);
            json!({
                "id":record.id,
                "kind":record.kind,
                "title":record.title,
                "preview":preview,
                "preview_truncated":preview_truncated,
                "source":record.source,
                "author":record.author,
                "date":record.date
            })
        })
        .collect()
}

fn get_tool(store: &db::Store, args: &Value, as_of: i64) -> ToolResult {
    let scope = explicit_scope(args)?;
    let id = required_string(args, "id", MAX_ID_BYTES)?;
    let resolution = store
        .get_current_evidence_in_scope_at(id, scope, as_of, MAX_SUPERSESSION_CHAIN)
        .map_err(ToolError::internal)?;
    let payload = serde_json::to_value(&resolution).map_err(ToolError::internal)?;
    if tool_wire_size(&payload)? > MAX_RESPONSE_BYTES {
        return Err(ToolError::new(
            "response_too_large",
            "exact record cannot be returned within the response byte limit",
        ));
    }
    match resolution {
        CurrentRecordResolution::Ok { .. } => Ok(payload),
        CurrentRecordResolution::Error { .. } => Err(ToolError::resolution(payload)),
    }
}

fn history_tool(store: &db::Store, args: &Value, as_of: i64) -> ToolResult {
    let scope = explicit_scope(args)?;
    let id = required_string(args, "id", MAX_ID_BYTES)?;
    let cursor = optional_string(args, "cursor", MAX_ID_BYTES)?;
    let limit = match args.get("limit") {
        None => MAX_HISTORY_PAGE_RECORDS,
        Some(value) => {
            let Some(limit) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
                return Err(ToolError::new(
                    "invalid_arguments",
                    "`limit` must be an integer",
                ));
            };
            if !(1..=MAX_HISTORY_PAGE_RECORDS).contains(&limit) {
                return Err(ToolError::new(
                    "limit_exceeded",
                    format!("`limit` must be from 1 to {MAX_HISTORY_PAGE_RECORDS}"),
                ));
            }
            limit
        }
    };

    let resolution = store
        .get_rationale_history_at(id, scope, cursor, limit, as_of, MAX_SUPERSESSION_CHAIN)
        .map_err(ToolError::internal)?;
    let payload = serde_json::to_value(&resolution).map_err(ToolError::internal)?;
    if tool_wire_size(&payload)? > MAX_RESPONSE_BYTES {
        let oversized = RationaleHistoryResolution::Error {
            contract: RATIONALE_HISTORY_CONTRACT,
            as_of: db::epoch_to_iso(as_of),
            requested_id: id.to_owned(),
            code: RationaleHistoryErrorCode::ResponseTooLarge,
            message: "exact history page cannot be returned within the response byte limit"
                .to_owned(),
            retryable: false,
        };
        return Err(ToolError::resolution(
            serde_json::to_value(oversized).map_err(ToolError::internal)?,
        ));
    }
    match resolution {
        RationaleHistoryResolution::Ok { .. } => Ok(payload),
        RationaleHistoryResolution::Error { .. } => Err(ToolError::resolution(payload)),
    }
}

fn commit_links_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = required_exact_non_blank_string(args, "scope", MAX_AUTHORITY_BYTES)?;
    let commit = required_exact_non_blank_string(args, "commit", MAX_COMMIT_LINK_HASH_BYTES)?;
    let cursor = match optional_string(args, "cursor", MAX_ID_BYTES)? {
        Some(cursor) if cursor.trim().is_empty() => {
            return Err(ToolError::new(
                "invalid_arguments",
                "`cursor` must not be empty",
            ));
        }
        Some(cursor) => Some(cursor),
        None => None,
    };
    let limit = match args.get("limit") {
        None => MAX_COMMIT_LINKS_PAGE_RECORDS,
        Some(value) => {
            let Some(limit) = value.as_u64().and_then(|value| usize::try_from(value).ok()) else {
                return Err(ToolError::new(
                    "invalid_arguments",
                    "`limit` must be an integer",
                ));
            };
            if !(1..=MAX_COMMIT_LINKS_PAGE_RECORDS).contains(&limit) {
                return Err(ToolError::new(
                    "limit_exceeded",
                    format!("`limit` must be from 1 to {MAX_COMMIT_LINKS_PAGE_RECORDS}"),
                ));
            }
            limit
        }
    };

    let resolution = store
        .get_commit_links(scope, commit, cursor, limit)
        .map_err(ToolError::internal)?;
    let payload = serde_json::to_value(&resolution).map_err(ToolError::internal)?;
    if tool_wire_size(&payload)? > MAX_RESPONSE_BYTES {
        let oversized = CommitLinksResolution::Error {
            contract: COMMIT_LINKS_CONTRACT,
            code: CommitLinksErrorCode::ResponseTooLarge,
            message: "commit links response cannot be returned within the response byte limit"
                .to_owned(),
            retryable: false,
        };
        return Err(ToolError::resolution(
            serde_json::to_value(oversized).map_err(ToolError::internal)?,
        ));
    }
    match resolution {
        CommitLinksResolution::Ok { .. } => Ok(payload),
        CommitLinksResolution::Error { .. } => Err(ToolError::resolution(payload)),
    }
}

fn link_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let decision = required_string(args, "decision", MAX_ID_BYTES)?;
    let commit = required_string(args, "commit", MAX_COMMIT_LINK_HASH_BYTES)?;
    let subject = optional_string(args, "subject", MAX_COMMIT_LINK_SUBJECT_BYTES)?.unwrap_or("");
    let identity = match store.evidence_identity_in_scope(decision, scope) {
        Ok(EvidenceIdentityResolution::Ok { identity }) => identity,
        Ok(EvidenceIdentityResolution::Error { .. }) => {
            return Err(ToolError::new(
                "not_found",
                "record is unavailable in the requested scope",
            ));
        }
        Err(error) => {
            return Err(ToolError::with_retryable(
                "store_unavailable",
                "commit link store is unavailable",
                db::store_error_is_retryable(&error),
            ));
        }
    };
    match store.link_git_in_scope(&identity, commit, subject) {
        ScopedCommitLinkResolution::Ok { .. } => {
            Ok(json!({"status":"ok","scope":scope,"decision":decision,"commit":commit}))
        }
        ScopedCommitLinkResolution::Error {
            code, retryable, ..
        } => match code {
            ScopedCommitLinkErrorCode::InvalidRequest => Err(ToolError::new(
                "invalid_arguments",
                "commit link request is invalid",
            )),
            ScopedCommitLinkErrorCode::EvidenceUnavailable => Err(ToolError::new(
                "not_found",
                "record is unavailable in the requested scope",
            )),
            ScopedCommitLinkErrorCode::LinkConflict => Err(ToolError::new(
                "link_conflict",
                "commit link already exists with a different subject",
            )),
            ScopedCommitLinkErrorCode::StoreUnavailable => Err(ToolError::with_retryable(
                "store_unavailable",
                "commit link store is unavailable",
                retryable,
            )),
        },
    }
}

fn feedback_tool(store: &db::Store, args: &Value) -> ToolResult {
    let scope = explicit_scope(args)?;
    let id = required_string(args, "id", MAX_ID_BYTES)?;
    let helpful = args
        .get("helpful")
        .and_then(Value::as_bool)
        .ok_or_else(|| ToolError::new("invalid_arguments", "`helpful` must be a boolean"))?;
    if !store
        .record_belongs_to_scope(id, scope)
        .map_err(ToolError::internal)?
    {
        return Err(ToolError::new(
            "not_found",
            "record is unavailable in the requested scope",
        ));
    }
    let effectiveness = store
        .feedback(id, helpful)
        .map_err(ToolError::internal)?
        .ok_or_else(|| {
            ToolError::new(
                "not_current",
                "record is unavailable in the requested scope",
            )
        })?;
    Ok(json!({
        "status":"ok",
        "scope":scope,
        "id":id,
        "helpful":helpful,
        "effectiveness":effectiveness
    }))
}
