/*
 * embed/mod.rs
 * Project: sts-x
 * Description: Embedding engine — stub or ONNX depending on feature
 *
 * Default build: empty stub, no external deps.
 * With `--features semantic`: loads BGE-small-en-v1.5 via ONNX Runtime.
 */

use anyhow::Result;
use std::path::Path;

// ─── Always-available math helpers ───────────────────────────────────

/// L2 normalize a vector
pub fn normalize_l2(vec: &[f32]) -> Vec<f32> {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        vec.iter().map(|x| x / norm).collect()
    } else {
        vec.to_vec()
    }
}

/// Cosine similarity between two normalized vectors
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    dot.clamp(0.0, 1.0)
}

// ─── Embedding model: stub (default) ────────────────────────────────

#[cfg(not(feature = "semantic"))]
mod inner {
    use super::*;

    /// Stub embedding model — no-op, used when built without `semantic` feature.
    pub struct EmbeddingModel;

    impl EmbeddingModel {
        pub fn load(_model_path: &Path, _tokenizer_path: &Path, _dim: usize, _max_length: usize) -> Result<Self> {
            anyhow::bail!("sts-x was built without the `semantic` feature. Rebuild with `--features semantic` to enable ONNX embeddings.");
        }

        pub fn encode(&mut self, _text: &str) -> Result<Vec<f32>> {
            anyhow::bail!("Embedding not available in default build.");
        }

        pub fn encode_batch(&mut self, _texts: &[String]) -> Result<Vec<Vec<f32>>> {
            anyhow::bail!("Embedding not available in default build.");
        }

        pub fn dim(&self) -> usize {
            0
        }
    }
}

// ─── Embedding model: ONNX (semantic feature) ───────────────────────

#[cfg(feature = "semantic")]
mod inner {
    use super::*;
    use anyhow::Context;
    use ort::session::Session;
    use ort::value::Value as OrtValue;
    use tokenizers::Tokenizer;

    pub struct EmbeddingModel {
        session: Session,
        tokenizer: Tokenizer,
        dim: usize,
        max_length: usize,
    }

    impl EmbeddingModel {
        pub fn load(model_path: &Path, tokenizer_path: &Path, dim: usize, max_length: usize) -> Result<Self> {
            let session = Session::builder()?
                .commit_from_file(model_path)
                .context("Failed to load ONNX model")?;

            let tokenizer = Tokenizer::from_file(tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

            Ok(Self { session, tokenizer, dim, max_length })
        }

        pub fn encode(&mut self, text: &str) -> Result<Vec<f32>> {
            use ort::inputs;

            let encoding = self.tokenizer.encode(text, true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

            let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
            let mask: Vec<i64> = encoding.get_attention_mask().iter().map(|&m| m as i64).collect();
            let types: Vec<i64> = encoding.get_type_ids().iter().map(|&t| t as i64).collect();

            let seq_len = ids.len().min(self.max_length);
            let padded_len = self.max_length;

            let mut padded_ids = vec![0i64; padded_len];
            let mut padded_mask = vec![0i64; padded_len];
            let mut padded_types = vec![0i64; padded_len];
            for i in 0..seq_len {
                padded_ids[i] = ids[i];
                padded_mask[i] = mask[i];
                padded_types[i] = types[i];
            }
            let mask_for_pooling = padded_mask.clone();

            let input_tensor = OrtValue::from_array(([1usize, padded_len], padded_ids))?;
            let mask_tensor = OrtValue::from_array(([1usize, padded_len], padded_mask))?;
            let type_tensor = OrtValue::from_array(([1usize, padded_len], padded_types))?;

            let outputs = self.session.run(inputs!(
                "input_ids" => input_tensor,
                "attention_mask" => mask_tensor,
                "token_type_ids" => type_tensor,
            ))?;

            for output in outputs.iter() {
                if let Ok((shape, data)) = output.1.try_extract_tensor::<f32>() {
                    let total = data.len();
                    if total >= self.dim && shape.len() > 1 {
                        let slen = shape[1] as usize;
                        let mut pooled = vec![0.0f32; self.dim];
                        let mut count = 0usize;
                        for j in 0..slen.min(padded_len) {
                            if mask_for_pooling[j] > 0 {
                                for k in 0..self.dim {
                                    pooled[k] += data[j * self.dim + k];
                                }
                                count += 1;
                            }
                        }
                        if count > 0 {
                            for val in &mut pooled {
                                *val /= count as f32;
                            }
                            return Ok(normalize_l2(&pooled));
                        }
                    }
                }
            }

            Ok(vec![0.0f32; self.dim])
        }

        pub fn encode_batch(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
            let mut results = Vec::with_capacity(texts.len());
            for t in texts {
                results.push(self.encode(t)?);
            }
            Ok(results)
        }

        pub fn dim(&self) -> usize {
            self.dim
        }
    }
}

pub use inner::EmbeddingModel;
