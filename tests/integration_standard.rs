use open_why::integration::{IntegrationManifest, IntegrationMode};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load(relative: &str) -> IntegrationManifest {
    let bytes = fs::read(root().join(relative)).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn checked_in_manifests_are_valid_and_cover_both_modes() {
    let mcp = load("examples/integrations/mcp-stdio.json");
    let rust = load("examples/integrations/rust-library.json");
    assert_eq!(mcp.mode, IntegrationMode::McpStdio);
    assert_eq!(rust.mode, IntegrationMode::RustLibrary);
    mcp.validate().unwrap();
    rust.validate().unwrap();
}

#[test]
fn checked_in_schema_is_json_and_names_the_same_contract() {
    let schema: Value = serde_json::from_slice(
        &fs::read(root().join("spec/open-why.integration-v1.schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        schema.pointer("/properties/contract/const"),
        Some(&Value::String("open-why.integration/v1".to_owned()))
    );
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn conformance_binary_accepts_checked_in_manifests() {
    for relative in [
        "examples/integrations/mcp-stdio.json",
        "examples/integrations/rust-library.json",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_why-integration-check"))
            .arg(root().join(relative))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["status"], "ok");
    }
}

#[test]
fn unknown_manifest_fields_fail_closed() {
    let mut value = serde_json::to_value(load("examples/integrations/rust-library.json")).unwrap();
    value["plugin_code"] = Value::String("run-me".to_owned());
    let error = serde_json::from_value::<IntegrationManifest>(value).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

#[test]
fn nested_unknown_manifest_fields_fail_closed() {
    let mut value = serde_json::to_value(load("examples/integrations/rust-library.json")).unwrap();
    value["rust"]["plugin_path"] = Value::String("run-me".to_owned());
    let error = serde_json::from_value::<IntegrationManifest>(value).unwrap_err();
    assert!(error.to_string().contains("unknown field"));
}

fn probe_manifest(protocol_version: &str) -> (PathBuf, Value) {
    let mut value = serde_json::to_value(load("examples/integrations/mcp-stdio.json")).unwrap();
    value["mcp"]["command"] = Value::String(env!("CARGO_BIN_EXE_why").to_owned());
    value["mcp"]["protocol_version"] = Value::String(protocol_version.to_owned());
    let path = root().join("target").join(format!(
        "integration-probe-{}-{protocol_version}.json",
        std::process::id()
    ));
    fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    (path, value)
}

fn scratch_directories(scratch_root: &PathBuf) -> Vec<PathBuf> {
    fs::read_dir(scratch_root)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("open-why-conformance-"))
        })
        .collect()
}

#[test]
fn conformance_probe_exercises_the_real_mcp_lifecycle() {
    let (manifest, _) = probe_manifest("2024-11-05");
    let scratch_root = root().join("target/integration-scratch-success");
    fs::create_dir_all(&scratch_root).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_why-integration-check"))
        .arg(&manifest)
        .arg("--probe")
        .current_dir(root())
        .env("OPEN_WHY_CONFORMANCE_ROOT", &scratch_root)
        .output()
        .unwrap();
    let _ = fs::remove_file(manifest);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["probed"], true);
    assert!(scratch_directories(&scratch_root).is_empty());
    let _ = fs::remove_dir(scratch_root);
}

#[test]
fn failed_probe_removes_its_scratch_directory() {
    let scratch_root = root().join("target/integration-scratch-failure");
    fs::create_dir_all(&scratch_root).unwrap();
    let (manifest, _) = probe_manifest("2099-01-01");
    let output = Command::new(env!("CARGO_BIN_EXE_why-integration-check"))
        .arg(&manifest)
        .arg("--probe")
        .current_dir(root())
        .env("OPEN_WHY_CONFORMANCE_ROOT", &scratch_root)
        .output()
        .unwrap();
    let _ = fs::remove_file(manifest);
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(scratch_directories(&scratch_root).is_empty());
    let _ = fs::remove_dir(scratch_root);
}
