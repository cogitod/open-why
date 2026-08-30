use anyhow::{Context, Result};
use serde::Deserialize;

/// A pluggable text embedder. open-why ships the interface, not the model: the backend
/// (cogitod's p-embeddings, an OpenAI-compatible endpoint, or a local model) is supplied by
/// the operator via `OPEN_WHY_EMBED_URL`. With no embedder configured, search stays lexical-first.
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// An OpenAI-compatible embeddings endpoint (also serves cogitod's p-embeddings HTTP surface).
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
        Self { url, model, api_key }
    }
}

impl Embedder for HttpEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
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

/// Configure an embedder from the environment; `None` = lexical-first (the shipped default).
pub fn from_env() -> Option<Box<dyn Embedder>> {
    let url = std::env::var("OPEN_WHY_EMBED_URL").ok()?;
    let model = std::env::var("OPEN_WHY_EMBED_MODEL")
        .unwrap_or_else(|_| "text-embedding-3-small".to_string());
    let api_key = std::env::var("OPEN_WHY_EMBED_API_KEY").ok();
    Some(Box::new(HttpEmbedder::new(url, model, api_key)))
}
