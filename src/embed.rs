use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[cfg(feature = "local-embeddings")]
mod local;
#[cfg(feature = "local-embeddings")]
pub use local::LocalEmbedder;
mod model;
pub use model::{fetch_model, verify_model, MODEL_REVISION};

fn load_local(path: &Path) -> Result<Box<dyn Embedder>> {
    #[cfg(feature = "local-embeddings")]
    {
        Ok(Box::new(LocalEmbedder::new(path)?))
    }
    #[cfg(not(feature = "local-embeddings"))]
    {
        let _ = path;
        anyhow::bail!("local embeddings require the local-embeddings Cargo feature")
    }
}

/// A pluggable text embedder. open-why ships the interface, not the model: the backend is
/// either a local on-device model (`LocalEmbedder`, via `OPEN_WHY_EMBED_MODEL_PATH`) or an
/// OpenAI-compatible endpoint (`HttpEmbedder`, via `OPEN_WHY_EMBED_URL`). With neither
/// configured, search stays lexical-first.
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// An OpenAI-compatible embeddings endpoint.
pub struct HttpEmbedder {
    url: String,
    model: String,
    api_key: Option<String>,
}

#[derive(Deserialize)]
struct EmbedResponse {
    data: Vec<EmbedData>,
}

#[derive(Deserialize)]
struct EmbedData {
    embedding: Vec<f32>,
}

impl HttpEmbedder {
    pub fn new(url: String, model: String, api_key: Option<String>) -> Self {
        Self {
            url,
            model,
            api_key,
        }
    }
}

impl Embedder for HttpEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        crate::privacy::check_text(text)?;
        let body = serde_json::json!({ "model": self.model, "input": text });
        let mut req = ureq::post(&self.url).header("Content-Type", "application/json");
        if let Some(key) = &self.api_key {
            req = req.header("Authorization", &format!("Bearer {key}"));
        }
        let text = req
            .send(serde_json::to_string(&body)?)
            .with_context(|| format!("embedding request to {}", self.url))?
            .body_mut()
            .read_to_string()
            .context("embedding response")?;
        let resp: EmbedResponse = serde_json::from_str(&text).context("embedding response")?;
        resp.data
            .into_iter()
            .next()
            .map(|d| d.embedding)
            .context("empty embedding response")
    }
}

/// Cosine similarity, clamped to [0, 1]. Different lengths yield 0 (no vector signal).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || b.is_empty() || a.len() != b.len() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (u, v) in a.iter().zip(b.iter()) {
        dot += u * v;
        na += u * u;
        nb += v * v;
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    (dot / (na.sqrt() * nb.sqrt())).clamp(0.0, 1.0)
}

/// Configure an embedder from the environment. `OPEN_WHY_EMBED_MODEL_PATH` takes precedence and
/// selects the local on-device model; `OPEN_WHY_EMBED_URL` selects an OpenAI-compatible remote;
/// with neither, the fetched model cache (`why fetch-model`) is used when present (auto-fetched
/// when `OPEN_WHY_AUTO_FETCH=1`); `Ok(None)` = lexical-first (the shipped default). `Err` only
/// when a path was requested but failed to load.
pub fn from_env() -> Result<Option<Box<dyn Embedder>>> {
    if let Ok(dir) = std::env::var("OPEN_WHY_EMBED_MODEL_PATH") {
        if dir.trim().is_empty() {
            anyhow::bail!("OPEN_WHY_EMBED_MODEL_PATH is set but empty");
        }
        let embedder = load_local(Path::new(&dir))
            .with_context(|| format!("OPEN_WHY_EMBED_MODEL_PATH={dir}"))?;
        return Ok(Some(embedder));
    }
    let Some(url) = std::env::var("OPEN_WHY_EMBED_URL").ok() else {
        // Neither configured: fall back to the fetched model cache when present.
        if std::env::var("OPEN_WHY_AUTO_FETCH").ok().as_deref() == Some("1") {
            anyhow::ensure!(
                cfg!(feature = "local-embeddings"),
                "automatic model download requires the local-embeddings Cargo feature"
            );
        }
        let dir = model_cache_dir();
        #[cfg(feature = "local-embeddings")]
        let onnx = dir.join("onnx").join("model_quantized.onnx");
        if std::env::var("OPEN_WHY_AUTO_FETCH").ok().as_deref() == Some("1")
            && !dir.join("onnx/model_quantized.onnx").exists()
        {
            fetch_model()?;
        }
        #[cfg(feature = "local-embeddings")]
        if onnx.exists() {
            let embedder =
                load_local(&dir).with_context(|| format!("cached model at {}", dir.display()))?;
            return Ok(Some(embedder));
        }
        return Ok(None);
    };
    let model = std::env::var("OPEN_WHY_EMBED_MODEL")
        .unwrap_or_else(|_| "text-embedding-3-small".to_string());
    let api_key = std::env::var("OPEN_WHY_EMBED_API_KEY").ok();
    Ok(Some(Box::new(HttpEmbedder::new(url, model, api_key))))
}

/// Where the fetched local model lives (`~/.cache/open-why/models/Xenova/all-MiniLM-L6-v2`).
pub fn model_cache_dir() -> PathBuf {
    crate::store::cache_dir()
        .join("models")
        .join("Xenova")
        .join("all-MiniLM-L6-v2")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_is_bounded_and_symmetric() {
        assert_eq!(cosine(&[1.0, 0.0], &[1.0, 0.0]), 1.0);
        assert_eq!(cosine(&[1.0, 0.0], &[0.0, 1.0]), 0.0);
        assert_eq!(cosine(&[1.0, 0.0], &[]), 0.0);
        // Different lengths carry no vector signal.
        assert_eq!(cosine(&[1.0, 0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[cfg(feature = "local-embeddings")]
    #[test]
    #[ignore = "requires the pinned model; run scripts/evaluate-local.sh"]
    fn local_embedder_produces_normalized_384d_vectors() {
        let dir = std::env::var("OPEN_WHY_EMBED_MODEL_PATH")
            .expect("set OPEN_WHY_EMBED_MODEL_PATH to the verified model");
        let embedder = LocalEmbedder::new(Path::new(&dir)).unwrap();
        let v = embedder.embed("evidence bound decision recall").unwrap();
        assert_eq!(v.len(), 384);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "expected unit norm, got {norm}");
        // Deterministic: same input, same vector.
        let again = embedder.embed("evidence bound decision recall").unwrap();
        assert_eq!(v, again);
    }

    #[cfg(feature = "local-embeddings")]
    #[test]
    #[ignore = "manual diagnostic, not an acceptance test"]
    fn debug_cosine_table() {
        let dir = std::env::var("OPEN_WHY_EMBED_MODEL_PATH").unwrap();
        let embedder = LocalEmbedder::new(Path::new(&dir)).unwrap();
        let queries = [
            "memory capability map engine",
            "TencentDB agent memory harvest",
        ];
        let titles = [
            "Decision recall capability map: evidence exists but capture is incomplete",
            "Agent memory survey: separate proven behavior from proposed behavior",
            "research: agent memory as execution state separate from context windows",
        ];
        for q in queries {
            let qe = embedder.embed(q).unwrap();
            println!("QUERY: {q}");
            for t in titles {
                let te = embedder.embed(t).unwrap();
                println!("  cos={:.4}  {t}", cosine(&qe, &te));
            }
        }
    }
}
