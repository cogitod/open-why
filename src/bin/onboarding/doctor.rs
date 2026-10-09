use super::{check_identity, configured_identity, optional_env, store_guidance};
use anyhow::{ensure, Context, Result};
use open_why::{inspect_store, StoreCompatibility};
use std::path::Path;

pub fn doctor(database: Option<&Path>, repo: Option<&Path>) -> Result<()> {
    println!("open-why {}", env!("CARGO_PKG_VERSION"));
    let executable = std::env::current_exe().context("locate why executable")?;
    println!("Executable: {executable:?}");
    let default = open_why::default_path();
    let database = database.unwrap_or(&default);
    println!("Database: {database:?}");
    let mut failed = false;
    report("Store", check_store(database), &mut failed);
    report("Embeddings", check_embeddings(), &mut failed);
    if let Some(repo) = repo {
        report("Repository", check_repository(repo), &mut failed);
    }
    println!("Client connection: not checked; inspect MCP status in your client.");
    println!("Read-only check: no database writes, model loads, downloads, or service requests.");
    ensure!(
        !failed,
        "some checks failed or could not be verified; see recovery guidance above"
    );
    Ok(())
}

fn report(label: &str, result: Result<String>, failed: &mut bool) {
    match result {
        Ok(message) => println!("{label}: {message}"),
        Err(error) => {
            println!("{label}: CHECK FAILED: {error:#}");
            *failed = true;
        }
    }
}

fn check_store(path: &Path) -> Result<String> {
    ensure!(path.is_absolute(), "use an absolute database path");
    let expected = configured_identity()?;
    let state = inspect_store(path)
        .context("inspect the database path; check permissions and symbolic links")?;
    let StoreCompatibility::Compatible { identity } = &state else {
        anyhow::bail!("{}", store_guidance(&state));
    };
    check_identity(expected.as_deref(), &identity.store_instance_id)?;
    Ok(format!(
        "compatible; identity {}{}",
        identity.store_instance_id,
        if expected.is_some() {
            " (matches configuration)"
        } else {
            " (no identity override configured)"
        }
    ))
}

fn local_model(path: &Path, source: &str) -> Result<String> {
    ensure!(
        cfg!(feature = "local-embeddings"),
        "local embeddings require the local-embeddings Cargo feature"
    );
    for file in ["tokenizer.json", "onnx/model_quantized.onnx"] {
        ensure!(path.join(file).is_file(), "{source} model is missing {file}; restore the model files or remove the explicit model setting");
    }
    Ok(format!(
        "{source} local model files present; loading and inference not verified"
    ))
}

fn check_embeddings() -> Result<String> {
    if let Some(path) = optional_env("OPEN_WHY_EMBED_MODEL_PATH")? {
        ensure!(
            !path.trim().is_empty(),
            "OPEN_WHY_EMBED_MODEL_PATH is empty; unset it for default retrieval"
        );
        return local_model(Path::new(&path), "configured");
    }
    if let Some(url) = optional_env("OPEN_WHY_EMBED_URL")? {
        ensure!(
            !url.trim().is_empty(),
            "OPEN_WHY_EMBED_URL is empty; unset it for default retrieval"
        );
        // URLs can contain credentials. Report the mode without echoing the endpoint or key.
        return Ok(
            "remote endpoint configured; connectivity and inference not verified (no request sent)"
                .into(),
        );
    }
    let cache = open_why::embed::model_cache_dir();
    if cfg!(feature = "local-embeddings") && cache.join("onnx/model_quantized.onnx").exists() {
        return local_model(&cache, "cached");
    }
    if optional_env("OPEN_WHY_AUTO_FETCH")?.as_deref() == Some("1") {
        return Ok("automatic model download configured for normal startup; download not attempted or verified".into());
    }
    Ok("lexical retrieval; no model or API key required".into())
}

fn check_repository(repo: &Path) -> Result<String> {
    ensure!(repo.is_absolute(), "--repo requires an absolute path");
    let inside = open_why::miner::git(repo, &["rev-parse", "--is-inside-work-tree"])
        .context("check Git installation and repository path")?;
    ensure!(inside.trim() == "true", "choose a Git working tree");
    open_why::miner::git(repo, &["rev-parse", "--verify", "HEAD"])
        .context("repository needs at least one commit before indexing")?;
    Ok(format!(
        "valid Git working tree at {repo:?}; index freshness not checked"
    ))
}
