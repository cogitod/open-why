use super::Embedder;
use anyhow::Result;
use std::{path::Path, sync::Mutex};
const MAX_TEXT_LENGTH: usize = 2000;
const MAX_SEQ_LEN: usize = 512;

/// A local, on-device `Xenova/all-MiniLM-L6-v2` embedder loaded directly through ONNX Runtime
/// and the Hugging Face tokenizer. It follows the standard sentence-transformer pipeline:
/// truncate to 2000 characters, mean-pool over the attention mask (CLS/SEP included), then
/// L2-normalize into the model's 384-dimensional vector space.
pub struct LocalEmbedder {
    tokenizer: tokenizers::Tokenizer,
    session: Mutex<ort::session::Session>,
}

impl LocalEmbedder {
    pub fn new(model_dir: &Path) -> Result<Self> {
        super::verify_model(model_dir)?;
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
        let attention_mask_t = ort::value::Tensor::from_array(([1usize, seq], attention_mask))
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
