use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::Deserialize;

/// Maximum characters fed to the tokenizer. Mirrors cogitod's XenovaBackend MAX_TEXT_LENGTH
/// (2000 chars ≈ ~500 tokens, the MiniLM-L6 safe limit).
const MAX_TEXT_LENGTH: usize = 2000;
/// all-MiniLM-L6-v2 max sequence length (config.json `max_position_embeddings`).
const MAX_SEQ_LEN: usize = 512;

/// A pluggable text embedder. open-why ships the interface, not the model: the backend is
/// either a local on-device model (`LocalEmbedder`, via `OPEN_WHY_EMBED_MODEL_PATH`) or an
/// OpenAI-compatible endpoint (`HttpEmbedder`, via `OPEN_WHY_EMBED_URL`). With neither
/// configured, search stays lexical-first.
pub trait Embedder: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
}

/// A local, on-device embedder: the same `Xenova/all-MiniLM-L6-v2` model cogitod runs, loaded
/// directly through onnxruntime + the HuggingFace tokenizer. Replicates Transformers.js
/// `feature-extraction` exactly — truncate to 2000 chars, mean-pool over the attention mask
/// (CLS/SEP included), then L2-normalize — so vectors land in the same 384-dim space as
/// cogitod's Xenova backend. This is what keeps TS cogitod and Rust open-why comparable.
pub struct LocalEmbedder {
    tokenizer: tokenizers::Tokenizer,
    session: Mutex<ort::session::Session>,
}

impl LocalEmbedder {
    pub fn new(model_dir: &Path) -> Result<Self> {
        let tokenizer_path = model_dir.join("tokenizer.json");
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("load tokenizer {}: {e}", tokenizer_path.display()))?;
        let model_path = model_dir.join("onnx").join("model_quantized.onnx");
        let builder = ort::session::Session::builder()
            .map_err(|e| anyhow::anyhow!("ort session builder: {e}"))?;
        let builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow::anyhow!("ort optimization level: {e}"))?;
        let mut builder = builder
            .with_intra_threads(1)
            .map_err(|e| anyhow::anyhow!("ort intra threads: {e}"))?;
        let session = builder
            .commit_from_file(&model_path)
            .map_err(|e| anyhow::anyhow!("ort load model {}: {e}", model_path.display()))?;
        Ok(Self {
            tokenizer,
            session: Mutex::new(session),
        })
    }
}

impl Embedder for LocalEmbedder {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let safe: &str = if text.len() > MAX_TEXT_LENGTH {
            let mut end = MAX_TEXT_LENGTH;
            while end > 0 && !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        } else {
            text
        };
        if safe.trim().is_empty() {
            anyhow::bail!("cannot embed empty text");
        }

        let mut encoding = self
            .tokenizer
            .encode(safe, true)
            .map_err(|e| anyhow::anyhow!("tokenize input: {e}"))?;
        if encoding.len() > MAX_SEQ_LEN {
            encoding.truncate(
                MAX_SEQ_LEN,
                0,
                tokenizers::utils::truncation::TruncationDirection::Right,
            );
        }

        let ids: Vec<i64> = encoding.get_ids().iter().map(|&x| x as i64).collect();
        let attention_mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .map(|&x| x as i64)
            .collect();
        let type_ids: Vec<i64> = encoding.get_type_ids().iter().map(|&x| x as i64).collect();
        let seq = ids.len();
        if seq == 0 {
            anyhow::bail!("empty token sequence");
        }

        let input_ids = ort::value::Tensor::from_array(([1usize, seq], ids))
            .map_err(|e| anyhow::anyhow!("build input_ids tensor: {e}"))?;
        let attention_mask_t =
            ort::value::Tensor::from_array(([1usize, seq], attention_mask))
                .map_err(|e| anyhow::anyhow!("build attention_mask tensor: {e}"))?;
        let token_type_ids = ort::value::Tensor::from_array(([1usize, seq], type_ids))
            .map_err(|e| anyhow::anyhow!("build token_type_ids tensor: {e}"))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow::anyhow!("embedder session lock poisoned"))?;
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids,
                "attention_mask" => attention_mask_t,
                "token_type_ids" => token_type_ids,
            ])
            .map_err(|e| anyhow::anyhow!("ort run: {e}"))?;

        let (_shape, data) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|e| anyhow::anyhow!("extract last_hidden_state: {e}"))?;
        let dim = data.len() / seq;
        if dim == 0 {
            anyhow::bail!("empty embedding output");
        }

        // Mean-pool over non-padded positions (CLS/SEP included), mirroring Transformers.js
        // `pooling: 'mean'`, then L2-normalize (`normalize: true`).
        let mask = encoding.get_attention_mask();
        let mut mean = vec![0.0f32; dim];
        let mut count = 0usize;
        for (i, &m) in mask.iter().enumerate().take(seq) {
            if m == 1 {
                let row = &data[i * dim..(i + 1) * dim];
                for (acc, &val) in mean.iter_mut().zip(row.iter()) {
                    *acc += val;
                }
                count += 1;
            }
        }
        if count == 0 {
            anyhow::bail!("no non-padded tokens");
        }
        for v in mean.iter_mut() {
            *v /= count as f32;
        }
        let norm: f32 = mean.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in mean.iter_mut() {
                *v /= norm;
            }
        }
        Ok(mean)
    }
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
        Self {
            url,
            model,
            api_key,
        }
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
        let embedder = LocalEmbedder::new(Path::new(&dir))
            .with_context(|| format!("OPEN_WHY_EMBED_MODEL_PATH={dir}"))?;
        return Ok(Some(Box::new(embedder)));
    }
    let Some(url) = std::env::var("OPEN_WHY_EMBED_URL").ok() else {
        // Neither configured: fall back to the fetched model cache when present.
        let dir = model_cache_dir();
        let onnx = dir.join("onnx").join("model_quantized.onnx");
        if !onnx.exists() && std::env::var("OPEN_WHY_AUTO_FETCH").ok().as_deref() == Some("1") {
            fetch_model()?;
        }
        if onnx.exists() {
            let embedder = LocalEmbedder::new(&dir)
                .with_context(|| format!("cached model at {}", dir.display()))?;
            return Ok(Some(Box::new(embedder)));
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

const MODEL_FILES: [&str; 3] = ["tokenizer.json", "config.json", "onnx/model_quantized.onnx"];
const MODEL_BASE_URL: &str = "https://huggingface.co/Xenova/all-MiniLM-L6-v2/resolve/main/";

/// Download the `Xenova/all-MiniLM-L6-v2` model files into the cache, skipping any already
/// present. Returns the model directory on success.
pub fn fetch_model() -> Result<PathBuf> {
    let dir = model_cache_dir();
    for f in MODEL_FILES {
        let dest = dir.join(f);
        if dest.exists() {
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let url = format!("{MODEL_BASE_URL}{f}");
        let bytes = ureq::get(&url)
            .call()
            .with_context(|| format!("download {url}"))?
            .body_mut()
            .with_config()
            .limit(100 * 1024 * 1024) // model_quantized.onnx is ~23MB, well past ureq's 10MB default
            .read_to_vec()
            .with_context(|| format!("read {url}"))?;
        std::fs::write(&dest, &bytes).with_context(|| format!("write {}", dest.display()))?;
    }
    Ok(dir)
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

    #[test]
    fn local_embedder_produces_normalized_384d_vectors() {
        let Ok(dir) = std::env::var("OPEN_WHY_EMBED_MODEL_PATH") else {
            return;
        };
        let embedder = LocalEmbedder::new(Path::new(&dir)).unwrap();
        let v = embedder.embed("open-why cogitod dependency inversion").unwrap();
        assert_eq!(v.len(), 384);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "expected unit norm, got {norm}");
        // Deterministic: same input, same vector.
        let again = embedder.embed("open-why cogitod dependency inversion").unwrap();
        assert_eq!(v, again);
    }

    #[test]
    #[ignore]
    fn debug_cosine_table() {
        let dir = std::env::var("OPEN_WHY_EMBED_MODEL_PATH").unwrap();
        let embedder = LocalEmbedder::new(Path::new(&dir)).unwrap();
        let queries = [
            "memory capability map engine",
            "TencentDB agent memory harvest",
        ];
        let titles = [
            "Cogito memory capability map — engine is real, capture wiring is the gap",
            "TencentDB-Agent-Memory harvest: what is already in cogitod and what is not",
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
