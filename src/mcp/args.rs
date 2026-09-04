use super::catalog::import_row_schema;
use super::common::{
    ToolError, MAX_AUTHORITY_BYTES, MAX_BODY_BYTES, MAX_GIT_REFS, MAX_ID_BYTES, MAX_TITLE_BYTES,
};
use crate::db;
use crate::store::{
    ExternalDecision, MAX_COMMIT_LINK_HASH_BYTES, MAX_COMMIT_LINK_SUBJECT_BYTES,
    MAX_TEMPORAL_VALUE_BYTES,
};
use serde_json::Value;
use std::path::Path;

pub(super) fn required_string<'a>(
    args: &'a Value,
    key: &str,
    max_bytes: usize,
) -> std::result::Result<&'a str, ToolError> {
    let Some(value) = args.get(key).and_then(Value::as_str) else {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` is required and must be a string"),
        ));
    };
    if value.is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` must not be empty"),
        ));
    }
    if value.len() > max_bytes {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("`{key}` exceeds {max_bytes} UTF-8 bytes"),
        ));
    }
    Ok(value)
}

pub(super) fn required_exact_non_blank_string<'a>(
    args: &'a Value,
    key: &str,
    max_bytes: usize,
) -> std::result::Result<&'a str, ToolError> {
    let Some(value) = args.get(key).and_then(Value::as_str) else {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` is required and must be a string"),
        ));
    };
    if value.len() > max_bytes {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("`{key}` exceeds {max_bytes} UTF-8 bytes"),
        ));
    }
    if value.trim().is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` must not be empty"),
        ));
    }
    Ok(value)
}

pub(super) fn optional_string<'a>(
    args: &'a Value,
    key: &str,
    max_bytes: usize,
) -> std::result::Result<Option<&'a str>, ToolError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.len() <= max_bytes => Ok(Some(value)),
        Some(Value::String(_)) => Err(ToolError::new(
            "limit_exceeded",
            format!("`{key}` exceeds {max_bytes} UTF-8 bytes"),
        )),
        Some(_) => Err(ToolError::new(
            "invalid_arguments",
            format!("`{key}` must be a string"),
        )),
    }
}

pub(super) fn explicit_repo(args: &Value) -> std::result::Result<&str, ToolError> {
    let repo = required_string(args, "repo", MAX_AUTHORITY_BYTES)?;
    if !Path::new(repo).is_absolute() {
        return Err(ToolError::new(
            "repository_authority_required",
            "`repo` must be an explicit absolute path",
        ));
    }
    Ok(repo)
}

pub(super) fn explicit_scope(args: &Value) -> std::result::Result<&str, ToolError> {
    required_string(args, "scope", MAX_AUTHORITY_BYTES).map_err(|error| {
        if error.payload["code"] == "invalid_arguments" {
            ToolError::new(
                "scope_authority_required",
                "an explicit non-empty `scope` is required",
            )
        } else {
            error
        }
    })
}

pub(super) fn kinds_from(args: &Value) -> std::result::Result<Vec<String>, ToolError> {
    let Some(value) = args.get("type") else {
        return Ok(Vec::new());
    };
    let kinds = match value {
        Value::String(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect(),
        Value::Array(values) => {
            let mut out = Vec::with_capacity(values.len());
            for value in values {
                let Some(value) = value.as_str() else {
                    return Err(ToolError::new(
                        "invalid_arguments",
                        "every `type` array entry must be a string",
                    ));
                };
                if !value.trim().is_empty() {
                    out.push(value.trim().to_owned());
                }
            }
            out
        }
        _ => {
            return Err(ToolError::new(
                "invalid_arguments",
                "`type` must be a string or string array",
            ))
        }
    };
    Ok(kinds)
}

pub(super) fn validate_import_row(
    store: &db::Store,
    row: &ExternalDecision,
    scope: &str,
) -> std::result::Result<(), ToolError> {
    if row.scope != scope {
        return Err(ToolError::new(
            "scope_mismatch",
            format!(
                "record `{}` does not belong to explicit scope `{scope}`",
                row.id
            ),
        ));
    }
    for (field, value, limit) in [
        ("id", row.id.as_str(), MAX_ID_BYTES),
        ("kind", row.kind.as_str(), 128),
        ("title", row.title.as_str(), MAX_TITLE_BYTES),
        ("content", row.content.as_str(), MAX_BODY_BYTES),
        ("source", row.source.as_str(), MAX_AUTHORITY_BYTES),
        ("author", row.author.as_str(), MAX_TITLE_BYTES),
        ("date", row.date.as_str(), MAX_TEMPORAL_VALUE_BYTES),
    ] {
        if value.len() > limit {
            return Err(ToolError::new(
                "limit_exceeded",
                format!(
                    "record `{}` field `{field}` exceeds {limit} UTF-8 bytes",
                    row.id
                ),
            ));
        }
    }
    if row.id.is_empty() || row.title.is_empty() || row.content.is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            "import record id, title, and content must not be empty",
        ));
    }
    if row.kind.is_empty() {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("record `{}` kind must not be empty", row.id),
        ));
    }
    if !row.importance.is_finite() || !(0.0..=1.0).contains(&row.importance) {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("record `{}` has invalid importance", row.id),
        ));
    }
    if row
        .effectiveness
        .is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(ToolError::new(
            "invalid_arguments",
            format!("record `{}` has invalid effectiveness", row.id),
        ));
    }
    for (field, value, limit) in [
        (
            "updated_at",
            row.updated_at.as_deref(),
            MAX_TEMPORAL_VALUE_BYTES,
        ),
        ("tags", row.tags.as_deref(), MAX_BODY_BYTES),
        (
            "valid_from",
            row.valid_from.as_deref(),
            MAX_TEMPORAL_VALUE_BYTES,
        ),
        (
            "valid_until",
            row.valid_until.as_deref(),
            MAX_TEMPORAL_VALUE_BYTES,
        ),
        ("superseded_by", row.superseded_by.as_deref(), MAX_ID_BYTES),
        ("fact_key", row.fact_key.as_deref(), MAX_ID_BYTES),
    ] {
        if value.is_some_and(|value| value.len() > limit) {
            return Err(ToolError::new(
                "limit_exceeded",
                format!(
                    "record `{}` field `{field}` exceeds {limit} UTF-8 bytes",
                    row.id
                ),
            ));
        }
    }
    if row.git_refs.len() > MAX_GIT_REFS {
        return Err(ToolError::new(
            "limit_exceeded",
            format!("record `{}` exceeds {MAX_GIT_REFS} Git references", row.id),
        ));
    }
    for git_ref in &row.git_refs {
        if git_ref.commit_hash.len() > MAX_COMMIT_LINK_HASH_BYTES
            || git_ref.commit_subject.len() > MAX_COMMIT_LINK_SUBJECT_BYTES
        {
            return Err(ToolError::new(
                "limit_exceeded",
                format!("record `{}` contains an oversized Git reference", row.id),
            ));
        }
    }
    for (field, value) in [
        ("valid_from", row.valid_from.as_deref()),
        ("valid_until", row.valid_until.as_deref()),
    ] {
        if let Some(value) = value.filter(|value| !value.is_empty()) {
            if !store
                .temporal_value_is_valid(value)
                .map_err(ToolError::internal)?
            {
                return Err(ToolError::new(
                    "invalid_arguments",
                    format!("record `{}` has invalid `{field}`", row.id),
                ));
            }
        }
    }
    if let (Some(valid_from), Some(valid_until)) = (
        row.valid_from.as_deref().filter(|value| !value.is_empty()),
        row.valid_until.as_deref().filter(|value| !value.is_empty()),
    ) {
        let from: Option<i64> = store
            .temporal_epoch(valid_from)
            .map_err(ToolError::internal)?;
        let until: Option<i64> = store
            .temporal_epoch(valid_until)
            .map_err(ToolError::internal)?;
        if from.zip(until).is_some_and(|(from, until)| from >= until) {
            return Err(ToolError::new(
                "invalid_arguments",
                format!("record `{}` has a non-positive validity interval", row.id),
            ));
        }
    }
    Ok(())
}

pub(super) fn validate_import_shape(rows: &Value) -> std::result::Result<(), ToolError> {
    let Some(rows) = rows.as_array() else {
        return Ok(());
    };
    let schema = import_row_schema();
    let allowed = schema["properties"]
        .as_object()
        .expect("import row properties are objects");
    let git_ref_allowed = schema["properties"]["git_refs"]["items"]["properties"]
        .as_object()
        .expect("Git reference properties are objects");
    for (index, row) in rows.iter().enumerate() {
        let Some(row) = row.as_object() else {
            continue;
        };
        for key in row.keys() {
            if !allowed.contains_key(key) {
                return Err(ToolError::new(
                    "invalid_arguments",
                    format!("unknown field `{key}` in import row {index}"),
                ));
            }
        }
        if let Some(git_refs) = row.get("git_refs").and_then(Value::as_array) {
            for (ref_index, git_ref) in git_refs.iter().enumerate() {
                let Some(git_ref) = git_ref.as_object() else {
                    continue;
                };
                for key in git_ref.keys() {
                    if !git_ref_allowed.contains_key(key) {
                        return Err(ToolError::new(
                            "invalid_arguments",
                            format!(
                                "unknown field `{key}` in import row {index} Git reference {ref_index}"
                            ),
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
