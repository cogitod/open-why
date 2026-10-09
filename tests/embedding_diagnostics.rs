#[allow(dead_code)]
mod onboarding_support;
use onboarding_support::{failure, snapshot, success, Sandbox};
use open_why::Store;

#[test]
fn doctor_autofetch_matches_build_capability_without_downloading() {
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    drop(Store::open_with_store_instance_id(&db, "test:diagnostics").unwrap());
    let before = snapshot(&s.0);
    let output = s
        .command()
        .args(["doctor", "--db"])
        .arg(&db)
        .env("OPEN_WHY_AUTO_FETCH", "1")
        .output()
        .unwrap();
    if cfg!(feature = "local-embeddings") {
        success(&output);
        assert!(String::from_utf8_lossy(&output.stdout).contains("download not attempted"));
    } else {
        failure(&output, "local-embeddings Cargo feature");
    }
    assert_eq!(snapshot(&s.0), before);
}

#[cfg(feature = "local-embeddings")]
#[test]
fn doctor_rejects_incomplete_and_corrupt_models_without_loading_or_writes() {
    let s = Sandbox::new();
    let db = s.0.join("store.db");
    drop(Store::open_with_store_instance_id(&db, "test:diagnostics").unwrap());
    let model = s.0.join(".cache/open-why/models/Xenova/all-MiniLM-L6-v2");
    std::fs::create_dir_all(model.join("onnx")).unwrap();
    std::fs::write(model.join("tokenizer.json"), b"{}").unwrap();
    std::fs::write(model.join("onnx/model_quantized.onnx"), b"damaged").unwrap();
    for explicit in [false, true] {
        let mut command = s.command();
        command.args(["doctor", "--db"]).arg(&db);
        if explicit {
            command.env("OPEN_WHY_EMBED_MODEL_PATH", &model);
        }
        let before = snapshot(&s.0);
        failure(&command.output().unwrap(), "config.json");
        assert_eq!(snapshot(&s.0), before);
    }
    std::fs::write(model.join("config.json"), b"{}").unwrap();
    for explicit in [false, true] {
        let mut command = s.command();
        command.args(["doctor", "--db"]).arg(&db);
        if explicit {
            command.env("OPEN_WHY_EMBED_MODEL_PATH", &model);
        }
        let before = snapshot(&s.0);
        failure(&command.output().unwrap(), "SHA-256 mismatch");
        assert_eq!(snapshot(&s.0), before);
    }
}
