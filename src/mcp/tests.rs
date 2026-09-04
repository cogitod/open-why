use super::catalog::*;
use super::common::*;
use super::handlers::*;
use super::protocol::*;
use crate::db;
use crate::store::{
    self, ExternalDecision, Record, COMMIT_LINKS_CONTRACT, MAX_COMMIT_LINK_HASH_BYTES,
    MAX_COMMIT_LINK_SUBJECT_BYTES, MAX_TEMPORAL_VALUE_BYTES, RATIONALE_HISTORY_CONTRACT,
    RATIONALE_IMPORT_CONTRACT,
};
use serde_json::{json, Value};

fn temp_store() -> db::Store {
    let path = std::fs::canonicalize(std::env::temp_dir())
        .unwrap()
        .join(format!(
            "open-why-mcp-unit-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
    db::Store::open_with_store_instance_id(&path, "provider:mcp-unit").unwrap()
}

#[test]
fn registry_names_are_unique_and_dispatch_uses_the_same_registry() {
    let mut names: Vec<&str> = TOOL_SPECS.iter().map(|spec| spec.name).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count);

    let store = temp_store();
    for spec in TOOL_SPECS {
        let result = dispatch_tool(&store, spec.name, &json!({}), 0);
        let error = result.expect_err("empty arguments must be typed errors");
        assert_ne!(error.payload["code"], "unknown_tool");
    }
    let unknown = dispatch_tool(&store, "open-why_hidden", &json!({}), 0).unwrap_err();
    assert_eq!(unknown.payload["code"], "unknown_tool");
}

#[test]
fn temporal_schema_and_runtime_share_the_canonical_byte_limit() {
    let expected = json!(MAX_TEMPORAL_VALUE_BYTES);
    let capture = input_schema(ToolKind::Capture);
    assert_eq!(capture["properties"]["valid_from"]["maxLength"], expected);
    let import = import_row_schema();
    for field in ["date", "updated_at", "valid_from", "valid_until"] {
        assert_eq!(import["properties"][field]["maxLength"], expected);
    }
    assert_eq!(registry_digest(), "c8e490fedb5ea771");

    let store = temp_store();
    let boundary = format!("2026-01-01T00:00:00.{}Z", "1".repeat(107));
    assert_eq!(boundary.len(), MAX_TEMPORAL_VALUE_BYTES);
    assert!(dispatch_tool(
        &store,
        "open-why_capture",
        &json!({
            "id":"mcp-time-boundary",
            "title":"MCP time boundary",
            "content":"canonical boundary",
            "scope":"scope-a",
            "valid_from":boundary
        }),
        0,
    )
    .is_ok());

    let over_bound = format!("2026-01-01T00:00:00.{}Z", "1".repeat(108));
    let oversized = dispatch_tool(
        &store,
        "open-why_capture",
        &json!({
            "id":"mcp-time-over-bound",
            "title":"MCP time over bound",
            "content":"must not persist",
            "scope":"scope-a",
            "valid_from":over_bound
        }),
        0,
    )
    .unwrap_err();
    assert_eq!(oversized.payload["code"], "limit_exceeded");

    let non_ascii = "é".repeat(MAX_TEMPORAL_VALUE_BYTES / 2);
    assert_eq!(non_ascii.len(), MAX_TEMPORAL_VALUE_BYTES);
    let noncanonical = dispatch_tool(
        &store,
        "open-why_capture",
        &json!({
            "id":"mcp-time-non-ascii",
            "title":"MCP time non ASCII",
            "content":"must not persist",
            "scope":"scope-a",
            "valid_from":non_ascii
        }),
        0,
    )
    .unwrap_err();
    assert_eq!(noncanonical.payload["code"], "invalid_arguments");
    assert_eq!(store.count_for_scope("scope-a").unwrap(), 1);
}

#[test]
fn malformed_json_and_invalid_arguments_are_protocol_or_tool_errors() {
    let store = temp_store();
    let input = b"{bad json}\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"open-why_get\",\"arguments\":{}}}\n";
    let mut output = Vec::new();
    serve_io(
        &std::sync::Mutex::new(store),
        &input[..],
        &mut output,
        || 1_700_000_000,
    )
    .unwrap();
    let lines: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines[0]["error"]["code"], -32700);
    assert_eq!(lines[1]["result"]["isError"], true);
    let payload: Value =
        serde_json::from_str(lines[1]["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(payload["code"], "scope_authority_required");
}

#[test]
fn server_clock_is_evaluated_for_each_request() {
    let store = temp_store();
    store
        .import_external(&[ExternalDecision {
            id: "clock-record".to_owned(),
            kind: "decision".to_owned(),
            title: "clock record".to_owned(),
            content: "complete rationale".to_owned(),
            importance: 0.5,
            source: "synthetic".to_owned(),
            author: "tester".to_owned(),
            date: "2026-01-01".to_owned(),
            updated_at: None,
            accessed_count: None,
            times_injected: None,
            effectiveness: None,
            tags: None,
            scope: "scope-a".to_owned(),
            valid_from: None,
            valid_until: None,
            superseded_by: None,
            fact_key: None,
            git_refs: Vec::new(),
        }])
        .unwrap();
    let request = |id| {
        json!({
            "jsonrpc":"2.0",
            "id":id,
            "method":"tools/call",
            "params":{
                "name":"open-why_get",
                "arguments":{"id":"clock-record","scope":"scope-a"}
            }
        })
    };
    let input = format!("{}\n{}\n", request(1), request(2));
    let now = std::cell::Cell::new(1_700_000_000_i64);
    let mut output = Vec::new();
    serve_io(
        &std::sync::Mutex::new(store),
        input.as_bytes(),
        &mut output,
        || {
            let instant = now.get();
            now.set(instant + 60);
            instant
        },
    )
    .unwrap();

    let payloads: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| {
            let response: Value = serde_json::from_str(line).unwrap();
            serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
                .unwrap()
        })
        .collect();
    assert_eq!(payloads[0]["as_of"], "2023-11-14T22:13:20Z");
    assert_eq!(payloads[1]["as_of"], "2023-11-14T22:14:20Z");
}

#[test]
fn exact_get_matches_scoped_store_resolution_for_a_foreign_successor() {
    let store = temp_store();
    let row = |id: &str, successor: Option<&str>, scope: &str| ExternalDecision {
        id: id.to_owned(),
        kind: "decision".to_owned(),
        title: format!("record {id}"),
        content: format!("private body for {id}"),
        importance: 0.5,
        source: "synthetic".to_owned(),
        author: "tester".to_owned(),
        date: "2026-01-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: scope.to_owned(),
        valid_from: Some("2026-01-01T00:00:00Z".to_owned()),
        valid_until: successor.map(|_| "2026-02-01T00:00:00Z".to_owned()),
        superseded_by: successor.map(str::to_owned),
        fact_key: None,
        git_refs: Vec::new(),
    };
    store
        .import_external(&[
            row("root", Some("middle"), "scope-a"),
            row("middle", Some("foreign-successor"), "scope-a"),
            row("foreign-successor", None, "scope-b"),
        ])
        .unwrap();
    let as_of = 1_772_323_200;
    let direct = serde_json::to_value(
        store
            .get_current_evidence_in_scope_at("root", "scope-a", as_of, 64)
            .unwrap(),
    )
    .unwrap();
    let error = dispatch_tool(
        &store,
        "open-why_get",
        &json!({"id":"root","scope":"scope-a"}),
        as_of,
    )
    .unwrap_err();

    assert_eq!(error.payload, direct);
    assert_eq!(error.payload["code"], "broken_chain");
    assert_eq!(
        error.payload["message"],
        "supersession chain is unavailable in the requested scope"
    );
    assert!(!serde_json::to_string(&error.payload)
        .unwrap()
        .contains("foreign-successor"));
}

#[test]
fn preview_is_utf8_safe_bounded_and_contains_stable_id() {
    let content = format!("{}x", "é".repeat(MAX_PREVIEW_BYTES));
    let record = Record {
        id: "stable-id".into(),
        kind: "decision".into(),
        title: "Unicode".into(),
        content,
        importance: 0.5,
        source: "synthetic".into(),
        author: "tester".into(),
        date: "2026-01-01".into(),
        commit_sha: String::new(),
        scope: "scope-a".into(),
        superseded_by: None,
        valid_from: None,
        valid_until: None,
        updated_at: String::new(),
        access_count: 0,
        effectiveness: 0.5,
        embedding: None,
    };
    let preview = previews(vec![record]).remove(0);
    assert_eq!(preview["id"], "stable-id");
    assert!(preview["preview"].as_str().unwrap().len() <= MAX_PREVIEW_BYTES);
    assert_eq!(preview["preview_truncated"], true);
}

#[test]
fn integration_bounds_and_unknown_arguments_fail_before_effects() {
    let store = temp_store();
    let oversized_query = "q".repeat(MAX_QUERY_BYTES + 1);
    let query_error = dispatch_tool(
        &store,
        "open-why_search",
        &json!({"query":oversized_query,"scope":"scope-a"}),
        0,
    )
    .unwrap_err();
    assert_eq!(query_error.payload["code"], "limit_exceeded");

    let limit_error = dispatch_tool(
        &store,
        "open-why_search",
        &json!({"query":"bounded","scope":"scope-a","limit":101}),
        0,
    )
    .unwrap_err();
    assert_eq!(limit_error.payload["code"], "limit_exceeded");

    let authority_error = dispatch_tool(
        &store,
        "open-why_index",
        &json!({"repo":"relative/path"}),
        0,
    )
    .unwrap_err();
    assert_eq!(
        authority_error.payload["code"],
        "repository_authority_required"
    );

    let unknown_argument = dispatch_tool(
        &store,
        "open-why_get",
        &json!({"id":"id","scope":"scope-a","unexpected_field":"forbidden"}),
        0,
    )
    .unwrap_err();
    assert_eq!(unknown_argument.payload["code"], "invalid_arguments");

    let oversized_body = "x".repeat(MAX_BODY_BYTES + 1);
    let capture_error = dispatch_tool(
        &store,
        "open-why_capture",
        &json!({"title":"title","content":oversized_body,"scope":"scope-a"}),
        0,
    )
    .unwrap_err();
    assert_eq!(capture_error.payload["code"], "limit_exceeded");
    assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

    let oversized_import = "x".repeat(MAX_IMPORT_BYTES);
    let import_error = dispatch_tool(
        &store,
        "open-why_import",
        &json!({"scope":"scope-a","rows":[{
            "id":"oversized-import","kind":"decision","title":"title",
            "content":oversized_import,"importance":0.5,"source":"test",
            "author":"tester","date":"2026-01-01","scope":"scope-a",
            "git_refs":[]
        }]}),
        0,
    )
    .unwrap_err();
    assert_eq!(import_error.payload["code"], "limit_exceeded");
    assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

    let git_refs: Vec<Value> = (0..=MAX_GIT_REFS)
        .map(|index| json!({"commit_hash":format!("{index:040x}"),"commit_subject":"subject"}))
        .collect();
    let refs_error = dispatch_tool(
        &store,
        "open-why_import",
        &json!({"scope":"scope-a","rows":[{
            "id":"too-many-refs","kind":"decision","title":"title",
            "content":"body","importance":0.5,"source":"test",
            "author":"tester","date":"2026-01-01","scope":"scope-a",
            "git_refs":git_refs
        }]}),
        0,
    )
    .unwrap_err();
    assert_eq!(refs_error.payload["code"], "limit_exceeded");
    assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

    let temporal_error = dispatch_tool(
        &store,
        "open-why_import",
        &json!({"scope":"scope-a","rows":[{
            "id":"inverted","kind":"decision","title":"title","content":"body",
            "scope":"scope-a","valid_from":"2026-02-01T00:00:00Z",
            "valid_until":"2026-01-01T00:00:00Z"
        }]}),
        0,
    )
    .unwrap_err();
    assert_eq!(temporal_error.payload["code"], "invalid_arguments");
    assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);

    let unknown_row_field = dispatch_tool(
        &store,
        "open-why_import",
        &json!({"scope":"scope-a","rows":[{
            "id":"unknown-field","kind":"decision","title":"title","content":"body",
            "scope":"scope-a","unexpected_field":"forbidden"
        }]}),
        0,
    )
    .unwrap_err();
    assert_eq!(unknown_row_field.payload["code"], "invalid_arguments");
    assert_eq!(store.count_for_scope("scope-a").unwrap(), 0);
}

#[test]
fn public_import_reports_exact_replay_and_typed_identity_conflict() {
    let store = temp_store();
    let original = json!({
        "id":"stable-import","kind":"decision","title":"stable title",
        "content":"stable body","importance":0.5,"source":"test",
        "author":"tester","date":"2026-01-01","scope":"scope-a",
        "git_refs":[{"commit_hash":"original","commit_subject":"Original"}]
    });
    let args = json!({"scope":"scope-a","rows":[original.clone()]});
    assert_eq!(
        dispatch_tool(&store, "open-why_import", &args, 0).unwrap()["imported"],
        1
    );
    assert_eq!(
        dispatch_tool(&store, "open-why_import", &args, 0).unwrap()["imported"],
        0
    );

    let mut conflict = original;
    conflict["content"] = json!("changed body");
    conflict["git_refs"] = json!([{
        "commit_hash":"must-not-append","commit_subject":"Must not append"
    }]);
    let error = dispatch_tool(
        &store,
        "open-why_import",
        &json!({"scope":"scope-a","rows":[conflict]}),
        0,
    )
    .unwrap_err();
    assert_eq!(error.payload["contract"], RATIONALE_IMPORT_CONTRACT);
    assert_eq!(error.payload["code"], "identity_conflict");
    assert_eq!(
        error.payload["message"],
        "record identity conflicts with sealed evidence"
    );
    assert!(!error.payload["message"]
        .as_str()
        .unwrap()
        .contains("changed body"));
    assert_eq!(store.linked_commits("stable-import").unwrap().len(), 1);
}

#[test]
fn exact_get_fails_typed_instead_of_truncating_an_oversized_wire_response() {
    let store = temp_store();
    let record = ExternalDecision {
        id: "large-record".to_owned(),
        kind: "decision".to_owned(),
        title: "large record".to_owned(),
        content: "\0".repeat(750_000),
        importance: 0.5,
        source: "synthetic".to_owned(),
        author: "tester".to_owned(),
        date: "2026-01-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: "scope-a".to_owned(),
        valid_from: None,
        valid_until: None,
        superseded_by: None,
        fact_key: None,
        git_refs: Vec::new(),
    };
    store.import_external(&[record]).unwrap();

    let error = dispatch_tool(
        &store,
        "open-why_get",
        &json!({"id":"large-record","scope":"scope-a"}),
        1_700_000_000,
    )
    .unwrap_err();
    assert_eq!(error.payload["contract"], MCP_ERROR_CONTRACT);
    assert_eq!(error.payload["code"], "response_too_large");
    assert_eq!(error.payload["as_of"], "2023-11-14T22:13:20Z");
}

#[test]
fn exact_history_enforces_page_bounds_and_returns_typed_oversize_failure() {
    let store = temp_store();
    let record = ExternalDecision {
        id: "large-history-record".to_owned(),
        kind: "decision".to_owned(),
        title: "large history record".to_owned(),
        content: "\0".repeat(750_000),
        importance: 0.5,
        source: "synthetic".to_owned(),
        author: "tester".to_owned(),
        date: "2026-01-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: "scope-a".to_owned(),
        valid_from: None,
        valid_until: None,
        superseded_by: None,
        fact_key: None,
        git_refs: Vec::new(),
    };
    store.import_external(&[record]).unwrap();

    for invalid_limit in [json!(0), json!(4), json!("3")] {
        let error = dispatch_tool(
            &store,
            "open-why_history",
            &json!({
                "id":"large-history-record",
                "scope":"scope-a",
                "limit":invalid_limit
            }),
            1_700_000_000,
        )
        .unwrap_err();
        assert!(matches!(
            error.payload["code"].as_str(),
            Some("limit_exceeded" | "invalid_arguments")
        ));
    }
    let unknown = dispatch_tool(
        &store,
        "open-why_history",
        &json!({
            "id":"large-history-record",
            "scope":"scope-a",
            "unexpected":"forbidden"
        }),
        1_700_000_000,
    )
    .unwrap_err();
    assert_eq!(unknown.payload["code"], "invalid_arguments");

    let oversized = dispatch_tool(
        &store,
        "open-why_history",
        &json!({"id":"large-history-record","scope":"scope-a"}),
        1_700_000_000,
    )
    .unwrap_err();
    assert_eq!(oversized.payload["contract"], RATIONALE_HISTORY_CONTRACT);
    assert_eq!(oversized.payload["code"], "response_too_large");
    assert_eq!(oversized.payload["as_of"], "2023-11-14T22:13:20Z");
}

#[test]
fn exact_commit_links_preserve_whitespace_bearing_identities() {
    let store = temp_store();
    let scope = " scope-a ";
    let commit = " exact-hash ";
    for (id, subject) in [(" record-a ", "first"), (" record-b ", "second")] {
        store
            .import_external(&[ExternalDecision {
                id: id.to_owned(),
                kind: "decision".to_owned(),
                title: id.to_owned(),
                content: "body must not be returned".to_owned(),
                importance: 0.5,
                source: "synthetic".to_owned(),
                author: "tester".to_owned(),
                date: "2026-01-01".to_owned(),
                updated_at: None,
                accessed_count: None,
                times_injected: None,
                effectiveness: None,
                tags: None,
                scope: scope.to_owned(),
                valid_from: None,
                valid_until: None,
                superseded_by: None,
                fact_key: None,
                git_refs: vec![store::GitRef {
                    commit_hash: commit.to_owned(),
                    commit_subject: subject.to_owned(),
                }],
            }])
            .unwrap();
    }

    let first = dispatch_tool(
        &store,
        "open-why_commit_links",
        &json!({"scope":scope,"commit":commit,"limit":1}),
        0,
    )
    .unwrap();
    assert_eq!(first["scope"], scope);
    assert_eq!(first["commit"], commit);
    assert_eq!(first["items"][0]["record_id"], " record-a ");
    assert_eq!(first["next_cursor"], " record-b ");

    for arguments in [
        json!({"scope":"scope-a","commit":commit}),
        json!({"scope":scope,"commit":"exact-hash"}),
    ] {
        let error = dispatch_tool(&store, "open-why_commit_links", &arguments, 0).unwrap_err();
        assert_eq!(error.payload["code"], "not_found");
    }

    let cursor = first["next_cursor"].as_str().unwrap();
    let second = dispatch_tool(
        &store,
        "open-why_commit_links",
        &json!({"scope":scope,"commit":commit,"limit":1,"cursor":cursor}),
        0,
    )
    .unwrap();
    assert_eq!(second["items"][0]["record_id"], " record-b ");
    assert_eq!(second["next_cursor"], Value::Null);

    let trimmed_cursor = dispatch_tool(
        &store,
        "open-why_commit_links",
        &json!({"scope":scope,"commit":commit,"cursor":"record-b"}),
        0,
    )
    .unwrap_err();
    assert_eq!(trimmed_cursor.payload["code"], "invalid_cursor");
}

#[test]
fn exact_commit_links_validate_bounds_and_fail_closed_on_oversize() {
    let store = temp_store();
    let record = ExternalDecision {
        id: "linked-record".to_owned(),
        kind: "decision".to_owned(),
        title: "linked record".to_owned(),
        content: "body must not be returned".to_owned(),
        importance: 0.5,
        source: "synthetic".to_owned(),
        author: "tester".to_owned(),
        date: "2026-01-01".to_owned(),
        updated_at: None,
        accessed_count: None,
        times_injected: None,
        effectiveness: None,
        tags: None,
        scope: "scope-a".to_owned(),
        valid_from: None,
        valid_until: None,
        superseded_by: None,
        fact_key: None,
        git_refs: vec![store::GitRef {
            commit_hash: "exact-hash".to_owned(),
            commit_subject: "s".repeat(MAX_COMMIT_LINK_SUBJECT_BYTES + 1),
        }],
    };
    store.import_external(&[record]).unwrap();

    for invalid_limit in [json!(0), json!(21), json!("20")] {
        let error = dispatch_tool(
            &store,
            "open-why_commit_links",
            &json!({"scope":"scope-a","commit":"exact-hash","limit":invalid_limit}),
            0,
        )
        .unwrap_err();
        assert!(matches!(
            error.payload["code"].as_str(),
            Some("limit_exceeded" | "invalid_arguments")
        ));
    }
    for (scope, commit) in [("   ", "exact-hash"), ("scope-a", "   ")] {
        let error = dispatch_tool(
            &store,
            "open-why_commit_links",
            &json!({"scope":scope,"commit":commit}),
            0,
        )
        .unwrap_err();
        assert_eq!(error.payload["code"], "invalid_arguments");
    }
    for arguments in [
        json!({"scope":" ".repeat(MAX_AUTHORITY_BYTES + 1),"commit":"exact-hash"}),
        json!({"scope":"scope-a","commit":" ".repeat(MAX_COMMIT_LINK_HASH_BYTES + 1)}),
        json!({"scope":"scope-a","commit":"exact-hash","cursor":" ".repeat(MAX_ID_BYTES + 1)}),
    ] {
        let error = dispatch_tool(&store, "open-why_commit_links", &arguments, 0).unwrap_err();
        assert_eq!(error.payload["code"], "limit_exceeded");
    }
    let unknown = dispatch_tool(
        &store,
        "open-why_commit_links",
        &json!({"scope":"scope-a","commit":"exact-hash","unexpected":true}),
        0,
    )
    .unwrap_err();
    assert_eq!(unknown.payload["code"], "invalid_arguments");

    let oversized = dispatch_tool(
        &store,
        "open-why_commit_links",
        &json!({"scope":"scope-a","commit":"exact-hash"}),
        0,
    )
    .unwrap_err();
    assert_eq!(oversized.payload["contract"], COMMIT_LINKS_CONTRACT);
    assert_eq!(oversized.payload["code"], "response_too_large");
    assert!(oversized.payload.get("content").is_none());
}
