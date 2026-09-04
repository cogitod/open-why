use super::*;

#[test]
fn exact_get_real_process_hides_cross_scope_identity_and_existence() {
    let foreign_path = unique_temp_db("scoped-current-foreign");
    let absent_path = unique_temp_db("scoped-current-absent");
    let scoped_record =
        |id: &str, successor: Option<&str>, scope: &str, content: &str| ExternalDecision {
            id: id.to_owned(),
            kind: "decision".to_owned(),
            title: format!("Rationale {id}"),
            content: content.to_owned(),
            importance: 0.8,
            source: "synthetic-fixture".to_owned(),
            author: "test-author".to_owned(),
            date: "2026-02-01".to_owned(),
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
            git_refs: vec![GitRef {
                commit_hash: format!("private-commit-{id}"),
                commit_subject: format!("Private evidence {id}"),
            }],
        };
    for path in [&foreign_path, &absent_path] {
        Store::open_with_store_instance_id(path, &provider_id_for(path))
            .unwrap()
            .import_external(&[
                scoped_record("cross-root", Some("middle"), "scope-a", "root body"),
                scoped_record("middle", Some("hidden-successor"), "scope-a", "middle body"),
            ])
            .unwrap();
    }
    Store::open_with_store_instance_id(&foreign_path, &provider_id_for(&foreign_path))
        .unwrap()
        .import_external(&[
            scoped_record(
                "hidden-successor",
                None,
                "scope-b",
                "foreign body 2099-01-01T00:00:00Z",
            ),
            scoped_record("same-root", None, "scope-b", "wrong-scope root body"),
        ])
        .unwrap();
    let corrupt = Connection::open(&foreign_path).unwrap();
    let identity_guard: String = corrupt
        .query_row(
            "SELECT sql FROM sqlite_schema
             WHERE type='trigger' AND name='decisions_identity_update_guard'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    corrupt
        .execute_batch("DROP TRIGGER decisions_identity_update_guard;")
        .unwrap();
    corrupt
        .execute(
            "UPDATE decisions
             SET content=CAST(zeroblob(8388608) AS TEXT), importance=X'FF', author=X'FF',
                 superseded_by=X'FF', valid_from=X'FF', valid_until=42
             WHERE id IN ('hidden-successor','same-root')",
            [],
        )
        .unwrap();
    corrupt.execute_batch(&identity_guard).unwrap();
    drop(corrupt);

    let foreign_watch = Connection::open(&foreign_path).unwrap();
    let absent_watch = Connection::open(&absent_path).unwrap();
    let data_version = |connection: &Connection| {
        connection
            .query_row("PRAGMA data_version", [], |row| row.get::<_, i64>(0))
            .unwrap()
    };
    let mut foreign_server = Server::spawn(&foreign_path);
    let mut absent_server = Server::spawn(&absent_path);
    initialize(&mut foreign_server, 100);
    initialize(&mut absent_server, 200);
    let foreign_version = data_version(&foreign_watch);
    let absent_version = data_version(&absent_watch);

    let normalize_as_of = |mut value: Value| {
        value["as_of"] = Value::String("normalized".to_owned());
        value
    };
    for (id, expected_code) in [("same-root", "not_found"), ("cross-root", "broken_chain")] {
        let (foreign, foreign_error) =
            foreign_server.call(300, "open-why_get", json!({"id":id,"scope":"scope-a"}));
        let (absent, absent_error) =
            absent_server.call(400, "open-why_get", json!({"id":id,"scope":"scope-a"}));
        assert!(foreign_error && absent_error);
        assert_eq!(foreign["contract"], "open-why.current-rationale/v1");
        assert_eq!(foreign["code"], expected_code);
        assert_eq!(normalize_as_of(foreign.clone()), normalize_as_of(absent));
        let wire = serde_json::to_string(&foreign).unwrap();
        for secret in [
            "hidden-successor",
            "2099-01-01T00:00:00Z",
            "foreign body",
            "private-commit-hidden-successor",
            "Private evidence hidden-successor",
            "wrong-scope root body",
        ] {
            assert!(!wire.contains(secret), "MCP error leaked {secret}");
        }
    }
    assert_eq!(data_version(&foreign_watch), foreign_version);
    assert_eq!(data_version(&absent_watch), absent_version);

    foreign_server.finish();
    absent_server.finish();
    std::fs::remove_dir_all(foreign_path.parent().unwrap()).unwrap();
    std::fs::remove_dir_all(absent_path.parent().unwrap()).unwrap();
}

#[test]
fn exact_id_contract_catalog_callability_and_fresh_process_digest() {
    let db_path = unique_temp_db("contract");
    let full_body = format!("final rationale sentinel\n{}", "body-data-".repeat(2_000));
    {
        let store =
            Store::open_with_store_instance_id(&db_path, &provider_id_for(&db_path)).unwrap();
        store
            .import_external(&[
                record(
                    "history-a",
                    "retired rationale α".to_owned(),
                    Some("history-b"),
                ),
                record(
                    "history-b",
                    "second rationale β".to_owned(),
                    Some("history-c"),
                ),
                record(
                    "history-c",
                    "third rationale γ".to_owned(),
                    Some("history-d"),
                ),
                record(
                    "history-d",
                    "fourth rationale δ".to_owned(),
                    Some("history-e"),
                ),
                record("history-e", full_body.clone(), None),
            ])
            .unwrap();
    }

    let mut server = Server::spawn(&db_path);
    let (digest, tools_json) = initialize(&mut server, 1);
    assert_eq!(digest, "b37082e4965e9009");
    let tools: Vec<Value> = serde_json::from_str(&tools_json).unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "open-why_ask",
            "open-why_index",
            "open-why_capture",
            "open-why_import",
            "open-why_search",
            "open-why_get",
            "open-why_history",
            "open-why_commit_links",
            "open-why_link",
            "open-why_feedback",
        ]
    );
    let tool = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .expect("catalog contains named tool")
    };
    assert_eq!(
        tool("open-why_capture")["inputSchema"]["properties"]["valid_from"]["maxLength"],
        128
    );
    let import_properties =
        &tool("open-why_import")["inputSchema"]["properties"]["rows"]["items"]["properties"];
    for field in ["date", "updated_at", "valid_from", "valid_until"] {
        assert_eq!(import_properties[field]["maxLength"], 128);
    }

    let repo = env!("CARGO_MANIFEST_DIR");
    let valid_calls = [
        ("open-why_index", json!({"repo":repo})),
        (
            "open-why_ask",
            json!({"question":"why evidence","repo":repo}),
        ),
        (
            "open-why_capture",
            json!({"id":"captured-id","title":"Captured","content":"bounded body","scope":"scope-a"}),
        ),
        ("open-why_import", json!({"scope":"scope-a","rows":[]})),
        (
            "open-why_search",
            json!({"query":"final rationale sentinel","scope":"scope-a","limit":10}),
        ),
        (
            "open-why_history",
            json!({"id":"history-a","scope":"scope-a","limit":3}),
        ),
        (
            "open-why_link",
            json!({"commit":"fedcba9876543210","decision":"history-e","scope":"scope-a"}),
        ),
        (
            "open-why_commit_links",
            json!({"commit":"fedcba9876543210","scope":"scope-a","limit":20}),
        ),
        (
            "open-why_feedback",
            json!({"id":"history-e","helpful":true,"scope":"scope-a"}),
        ),
        ("open-why_get", json!({"id":"history-a","scope":"scope-a"})),
    ];
    let mut payloads = Vec::new();
    for (offset, (name, arguments)) in valid_calls.into_iter().enumerate() {
        let (payload, is_error) = server.call(10 + offset as u64, name, arguments);
        assert!(!is_error, "advertised tool {name} failed: {payload}");
        payloads.push((name, payload));
    }

    let search = &payloads[4].1;
    assert_eq!(search["results"][0]["id"], "history-e");
    assert_eq!(search["results"][0]["preview_truncated"], true);
    assert!(search["results"][0]["preview"].as_str().unwrap().len() <= 512);

    let history = &payloads[5].1;
    assert_eq!(history["contract"], "open-why.rationale-history/v1");
    assert_eq!(history["requested_id"], "history-a");
    assert_eq!(history["page_start_id"], "history-a");
    assert_eq!(history["complete"], false);
    assert_eq!(history["next_cursor"], "history-d");
    let first_records = history["records"].as_array().unwrap();
    assert_eq!(first_records.len(), 3);
    assert_eq!(first_records[0]["record"]["content"], "retired rationale α");
    assert_eq!(first_records[1]["record"]["content"], "second rationale β");
    assert_eq!(first_records[2]["record"]["content"], "third rationale γ");
    assert!(first_records.iter().all(|item| item["git_refs"]
        .as_array()
        .is_some_and(|refs| refs.len() == 1)));

    let (history_second, history_second_error) = server.call(
        28,
        "open-why_history",
        json!({"id":"history-a","scope":"scope-a","limit":3,"cursor":"history-d"}),
    );
    assert!(!history_second_error);
    assert_eq!(history_second["page_start_id"], "history-d");
    assert_eq!(history_second["complete"], true);
    assert_eq!(history_second["next_cursor"], Value::Null);
    assert_eq!(
        history_second["records"][0]["record"]["content"],
        "fourth rationale δ"
    );
    assert_eq!(history_second["records"][1]["record"]["content"], full_body);
    assert!(history_second["records"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["git_refs"]
            .as_array()
            .is_some_and(|refs| !refs.is_empty())));
    let reconstructed: Vec<&str> = first_records
        .iter()
        .chain(history_second["records"].as_array().unwrap())
        .map(|item| item["record"]["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        reconstructed,
        [
            "history-a",
            "history-b",
            "history-c",
            "history-d",
            "history-e"
        ]
    );

    let history_tool = tools
        .iter()
        .find(|tool| tool["name"] == "open-why_history")
        .unwrap();
    assert_eq!(
        history_tool["inputSchema"]["required"],
        json!(["id", "scope"])
    );
    assert_eq!(
        history_tool["inputSchema"]["properties"]["limit"]["maximum"],
        3
    );
    assert_eq!(history_tool["inputSchema"]["additionalProperties"], false);

    let import_tool = tools
        .iter()
        .find(|tool| tool["name"] == "open-why_import")
        .unwrap();
    assert_eq!(
        import_tool["_meta"]["contract"],
        "open-why.rationale-import/v1"
    );
    assert!(tools
        .iter()
        .filter(|tool| tool["name"] != "open-why_import")
        .all(|tool| tool.get("_meta").is_none()));
    assert_eq!(payloads[3].1["contract"], "open-why.rationale-import/v1");
    assert_eq!(payloads[3].1["imported"], 0);

    let (history_repeat, history_repeat_error) = server.call(
        29,
        "open-why_history",
        json!({"id":"history-a","scope":"scope-a","limit":3}),
    );
    assert!(!history_repeat_error);
    let mut first_history_without_clock = history.clone();
    let mut repeat_history_without_clock = history_repeat;
    first_history_without_clock
        .as_object_mut()
        .unwrap()
        .remove("as_of");
    repeat_history_without_clock
        .as_object_mut()
        .unwrap()
        .remove("as_of");
    assert_eq!(first_history_without_clock, repeat_history_without_clock);

    let commit_links = &payloads[7].1;
    assert_eq!(commit_links["contract"], "open-why.commit-links/v1");
    assert_eq!(commit_links["status"], "ok");
    assert_eq!(commit_links["scope"], "scope-a");
    assert_eq!(commit_links["commit"], "fedcba9876543210");
    assert_eq!(commit_links["items"][0]["record_id"], "history-e");
    assert_eq!(commit_links["items"][0]["commit_subject"], "");
    assert_eq!(commit_links["next_cursor"], Value::Null);
    assert!(commit_links.get("content").is_none());

    let commit_links_tool = tools
        .iter()
        .find(|tool| tool["name"] == "open-why_commit_links")
        .unwrap();
    assert_eq!(
        commit_links_tool["inputSchema"]["required"],
        json!(["scope", "commit"])
    );
    assert_eq!(
        commit_links_tool["inputSchema"]["properties"]["limit"]["maximum"],
        20
    );
    assert_eq!(
        commit_links_tool["inputSchema"]["additionalProperties"],
        false
    );

    let (historical_link, historical_link_error) = server.call(
        39,
        "open-why_commit_links",
        json!({"commit":"commit-history-a","scope":"scope-a"}),
    );
    assert!(!historical_link_error);
    assert_eq!(historical_link["items"][0]["record_id"], "history-a");

    let get = &payloads[9].1;
    assert_eq!(get["contract"], "open-why.current-rationale/v1");
    assert_eq!(get["requested_id"], "history-a");
    assert_eq!(get["current_id"], "history-e");
    assert_eq!(get["record"]["content"], full_body);
    assert!(get["record"]["updated_at"].is_string());
    assert!(get["record"]["access_count"].is_number());
    assert!(get["record"]["effectiveness"].is_number());
    assert!(get["git_refs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|git_ref| git_ref["commit_hash"] == "0123456789abcdef"));
    assert_eq!(
        get["supersession_chain"],
        json!([
            "history-a",
            "history-b",
            "history-c",
            "history-d",
            "history-e"
        ])
    );
    assert!(get["as_of"].as_str().unwrap().ends_with('Z'));

    let (repeat, repeat_error) = server.call(
        30,
        "open-why_get",
        json!({"id":"history-a","scope":"scope-a"}),
    );
    assert!(!repeat_error);
    let mut first_without_clock = get.clone();
    let mut repeat_without_clock = repeat.clone();
    first_without_clock.as_object_mut().unwrap().remove("as_of");
    repeat_without_clock
        .as_object_mut()
        .unwrap()
        .remove("as_of");
    assert_eq!(first_without_clock, repeat_without_clock);
    assert!(repeat["as_of"].as_str().unwrap().ends_with('Z'));

    let (wrong_scope, wrong_scope_error) = server.call(
        31,
        "open-why_get",
        json!({"id":"history-a","scope":"scope-b"}),
    );
    assert!(wrong_scope_error);
    assert_eq!(wrong_scope["code"], "not_found");
    assert!(wrong_scope["as_of"].as_str().unwrap().ends_with('Z'));
    let (missing, missing_error) = server.call(
        36,
        "open-why_get",
        json!({"id":"missing-id","scope":"scope-a"}),
    );
    assert!(missing_error);
    assert_eq!(missing["contract"], "open-why.current-rationale/v1");
    assert_eq!(missing["code"], "not_found");
    assert!(missing["as_of"].as_str().unwrap().ends_with('Z'));
    let (invalid_cursor, invalid_cursor_error) = server.call(
        37,
        "open-why_history",
        json!({"id":"history-a","scope":"scope-a","cursor":"captured-id"}),
    );
    assert!(invalid_cursor_error);
    assert_eq!(invalid_cursor["contract"], "open-why.rationale-history/v1");
    assert_eq!(invalid_cursor["code"], "invalid_cursor");
    let (unknown, unknown_error) = server.call(32, "open-why_hidden", json!({}));
    assert!(unknown_error);
    assert_eq!(unknown["code"], "unknown_tool");

    let malformed = server.raw("{bad json}");
    assert_eq!(malformed["error"]["code"], -32700);
    let method = server.request(json!({"jsonrpc":"2.0","id":34,"method":"hidden"}));
    assert_eq!(method["error"]["code"], -32601);
    let (invalid, invalid_error) = server.call(35, "open-why_get", json!({"id":"stale-id"}));
    assert!(invalid_error);
    assert_eq!(invalid["code"], "scope_authority_required");

    let import_row = json!({
        "id":"mcp-import","kind":"decision","title":"MCP import",
        "content":"sealed MCP body","importance":0.5,"source":"test",
        "author":"tester","date":"2026-01-01","scope":"scope-a",
        "git_refs":[{"commit_hash":"mcp-original","commit_subject":"Original"}]
    });
    let import_args = json!({"scope":"scope-a","rows":[import_row.clone()]});
    let (created_import, created_import_error) =
        server.call(50, "open-why_import", import_args.clone());
    assert!(!created_import_error);
    assert_eq!(created_import["contract"], "open-why.rationale-import/v1");
    assert_eq!(created_import["imported"], 1);
    let (replayed_import, replayed_import_error) = server.call(51, "open-why_import", import_args);
    assert!(!replayed_import_error);
    assert_eq!(replayed_import["imported"], 0);

    let mut conflict = import_row;
    conflict["content"] = json!("changed secret body");
    conflict["git_refs"] = json!([{
        "commit_hash":"must-not-append","commit_subject":"Must not append"
    }]);
    let new_row = json!({
        "id":"mcp-must-not-insert","kind":"decision","title":"Must not insert",
        "content":"must not insert","importance":0.5,"source":"test",
        "author":"tester","date":"2026-01-01","scope":"scope-a",
        "git_refs":[{"commit_hash":"must-not-land","commit_subject":"Must not land"}]
    });
    let (rejected_import, rejected_import_error) = server.call(
        52,
        "open-why_import",
        json!({"scope":"scope-a","rows":[new_row,conflict]}),
    );
    assert!(rejected_import_error);
    assert_eq!(rejected_import["contract"], "open-why.rationale-import/v1");
    assert_eq!(rejected_import["code"], "identity_conflict");
    assert_eq!(
        rejected_import["message"],
        "record identity conflicts with sealed evidence"
    );
    assert!(rejected_import["message"].as_str().unwrap().len() <= 128);
    assert!(!serde_json::to_string(&rejected_import)
        .unwrap()
        .contains("changed secret body"));
    let (preserved, preserved_error) = server.call(
        53,
        "open-why_get",
        json!({"id":"mcp-import","scope":"scope-a"}),
    );
    assert!(!preserved_error);
    assert_eq!(preserved["record"]["content"], "sealed MCP body");
    assert_eq!(preserved["git_refs"].as_array().unwrap().len(), 1);
    let (missing_batch_row, missing_batch_row_error) = server.call(
        54,
        "open-why_get",
        json!({"id":"mcp-must-not-insert","scope":"scope-a"}),
    );
    assert!(missing_batch_row_error);
    assert_eq!(missing_batch_row["code"], "not_found");
    server.finish();

    let mut fresh = Server::spawn(&db_path);
    let (fresh_digest, fresh_tools_json) = initialize(&mut fresh, 100);
    assert_eq!(digest, fresh_digest);
    assert_eq!(tools_json, fresh_tools_json);
    fresh.finish();

    std::fs::remove_dir_all(db_path.parent().unwrap()).unwrap();
}
