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

// ─── Semantic 开关与模型加载（P0-2）──────────────────────────────

/// 是否请求语义检索：环境变量 `STX_SEMANTIC=1|true`。
/// CLI 的 `--semantic` flag 由调用方合并（`flag || semantic_requested()`）。
pub fn semantic_requested() -> bool {
    std::env::var("STX_SEMANTIC")
        .map(|v| {
            let v = v.trim();
            v.eq_ignore_ascii_case("1") || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// 尝试加载 embedding 模型（仅当 `STX_SEMANTIC` 请求时）。
///
/// 模型目录查找顺序：`$STX_MODEL_DIR` > 可执行文件同目录 `models/` >
/// 系统缓存根 `{cache_root}/models/` > `config.model_path`。
/// 目录布局：`model.onnx + tokenizer.json` 平铺，或一层子目录
/// （`bge-small-zh-v1.5/`、`bge-small-en-v1.5/` 等任意目录名）。
///
/// 模型缺失/加载失败 → 记录 warn 并返回 None（优雅降级到 BM25 + P0-1 重试链，
/// 不崩溃）。模型文件不进 git（见 .gitignore `models/`）。
#[cfg(feature = "semantic")]
pub fn maybe_load_model(config: &crate::types::IndexConfig) -> Option<EmbeddingModel> {
    if !semantic_requested() {
        return None;
    }
    // DIM 仅作 load 时的 fallback 提示（encode 会从模型输出 shape 自适应校准：
    // bge-small-en 384 维 / bge-small-zh 512 维，这里给当前模型的 512）。
    const DIM: usize = 512;
    const MAX_LEN: usize = 512;

    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("STX_MODEL_DIR") {
        candidates.push(std::path::PathBuf::from(d));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            candidates.push(parent.join("models"));
        }
    }
    candidates.push(crate::cache::cache_root().join("models"));
    candidates.push(config.model_path.clone());

    for dir in candidates {
        if let Some((model, tok)) = find_model_files(&dir) {
            // load-dynamic 特性下 ort 找不到 dylib 会 PANIC（而非 Err）——
            // 必须在调用 ort 之前预检，保证模型/运行时缺失时优雅降级不崩溃。
            if !ort_runtime_ready() {
                tracing::warn!(
                    "Semantic model found at {} but ONNX Runtime dylib is missing. Set ORT_DYLIB_PATH to the libonnxruntime dylib (e.g. ORT_DYLIB_PATH=/path/libonnxruntime.1.28.0.dylib). Falling back to BM25 + auto-retry.",
                    dir.display()
                );
                return None;
            }
            return match EmbeddingModel::load(&model, &tok, DIM, MAX_LEN) {
                Ok(m) => {
                    tracing::info!("Semantic model loaded from {}", dir.display());
                    // 触发一次 encode，校准并打印真实维度（bge-en=384 / bge-zh=512）
                    let mut m = m;
                    if let Ok(v) = m.encode("sts-x dimension probe") {
                        tracing::info!("Embedding dimension: {} (en=384 / zh=512)", v.len());
                    }
                    Some(m)
                }
                Err(e) => {
                    tracing::warn!("Failed to load semantic model {}: {e:#}", model.display());
                    None
                }
            };
        }
    }

    tracing::warn!(
        "STX_SEMANTIC=1 but no embedding model found. Put model.onnx + tokenizer.json under $STX_MODEL_DIR, <exe>/models, or {} (flat, or in any one-level subdir). Falling back to BM25 + auto-retry.",
        crate::cache::cache_root().join("models").display()
    );
    None
}

/// 探测 ONNX Runtime 动态库是否可用。load-dynamic 特性下 ort 的 dlopen
/// 失败是 panic 而非 Err，所以这里做保守预检：显式 `ORT_DYLIB_PATH` 必须
/// 指向存在的文件；否则检查常见系统路径。预检漏判（dylib 在非常规路径）
/// 只会保守降级到 BM25，不会崩——用户设 ORT_DYLIB_PATH 即可显式放行。
#[cfg(feature = "semantic")]
fn ort_runtime_ready() -> bool {
    if let Ok(p) = std::env::var("ORT_DYLIB_PATH") {
        let p = p.trim();
        return !p.is_empty() && std::path::Path::new(p).exists();
    }
    #[cfg(target_os = "macos")]
    {
        for cand in [
            "/usr/local/lib/libonnxruntime.dylib",
            "/usr/lib/libonnxruntime.dylib",
            "/opt/homebrew/lib/libonnxruntime.dylib",
        ] {
            if std::path::Path::new(cand).exists() {
                return true;
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in ["onnxruntime.dll", "lib/onnxruntime.dll"] {
                    if dir.join(name).exists() {
                        return true;
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        for cand in [
            "/usr/lib/libonnxruntime.so",
            "/usr/local/lib/libonnxruntime.so",
        ] {
            if std::path::Path::new(cand).exists() {
                return true;
            }
        }
    }
    false
}

/// 在目录中查找 embedding 模型文件。支持三种布局（不再写死模型目录名，
/// `STX_MODEL_DIR` 可指向任意模型根，换模型无需改代码）：
///   1. 平铺：`<dir>/model.onnx + tokenizer.json`
///   2. 一层子目录：`<dir>/<任意名>/model.onnx + tokenizer.json`
///      （如 bge-small-zh-v1.5 / bge-small-en-v1.5 / all-MiniLM-L6-v2 …）
///   3. 目录本身是模型目录（`<dir>/` 内含 model.onnx），同样被平铺分支覆盖
#[cfg(feature = "semantic")]
fn find_model_files(dir: &std::path::Path) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    // 平铺布局优先
    let flat_m = dir.join("model.onnx");
    let flat_t = dir.join("tokenizer.json");
    if flat_m.exists() && flat_t.exists() {
        return Some((flat_m, flat_t));
    }
    // 扫描一层子目录（按名称排序保证确定性）
    let mut subs: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subs.sort();
    for d in subs {
        let m = d.join("model.onnx");
        let t = d.join("tokenizer.json");
        if m.exists() && t.exists() {
            return Some((m, t));
        }
    }
    None
}

/// 非 semantic 构建的占位：请求语义时给清晰指引，然后降级。
#[cfg(not(feature = "semantic"))]
pub fn maybe_load_model(_config: &crate::types::IndexConfig) -> Option<EmbeddingModel> {
    if semantic_requested() {
        tracing::warn!(
            "Semantic search requested (STX_SEMANTIC=1 / --semantic) but sts-x was built without the `semantic` feature. Rebuild with: cargo build --release --features semantic"
        );
    }
    None
}

// ─── Embedding model: stub (default) ────────────────────────────────

#[cfg(not(feature = "semantic"))]
mod inner {
    use super::*;

    /// Stub embedding model — no-op, used when built without `semantic` feature.
    pub struct EmbeddingModel;

    impl EmbeddingModel {
        pub fn load(
            _model_path: &Path,
            _tokenizer_path: &Path,
            _dim: usize,
            _max_length: usize,
        ) -> Result<Self> {
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
        pub fn load(
            model_path: &Path,
            tokenizer_path: &Path,
            dim: usize,
            max_length: usize,
        ) -> Result<Self> {
            let session = Session::builder()?
                .commit_from_file(model_path)
                .context("Failed to load ONNX model")?;

            let tokenizer = Tokenizer::from_file(tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {}", e))?;

            Ok(Self {
                session,
                tokenizer,
                dim,
                max_length,
            })
        }

        pub fn encode(&mut self, text: &str) -> Result<Vec<f32>> {
            use ort::inputs;

            let encoding = self
                .tokenizer
                .encode(text, true)
                .map_err(|e| anyhow::anyhow!("Tokenization failed: {}", e))?;

            let ids: Vec<i64> = encoding.get_ids().iter().map(|&id| id as i64).collect();
            let mask: Vec<i64> = encoding
                .get_attention_mask()
                .iter()
                .map(|&m| m as i64)
                .collect();
            let types: Vec<i64> = encoding.get_type_ids().iter().map(|&t| t as i64).collect();

            let seq_len = ids.len().min(self.max_length);
            let padded_len = self.max_length;

            let mut padded_ids = vec![0i64; padded_len];
            let mut padded_mask = vec![0i64; padded_len];
            let mut padded_types = vec![0i64; padded_len];
            padded_ids[..seq_len].copy_from_slice(&ids[..seq_len]);
            padded_mask[..seq_len].copy_from_slice(&mask[..seq_len]);
            padded_types[..seq_len].copy_from_slice(&types[..seq_len]);
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
                    let rank = shape.len();
                    // 自适应真实输出维度：shape = [batch, seq, hidden]。
                    // 不要硬编码 384/512 —— bge-small-en 是 384 维、bge-small-zh 是
                    // 512 维，硬编码会让 mean-pooling 跨 token 索引错位（垃圾向量）。
                    if rank >= 2 {
                        let hidden = shape[rank - 1] as usize;
                        let slen = shape[rank - 2] as usize;
                        let total = data.len();
                        if hidden > 0 && total >= hidden {
                            let mut pooled = vec![0.0f32; hidden];
                            let mut count = 0usize;
                            for j in 0..slen.min(padded_len) {
                                if mask_for_pooling[j] > 0 {
                                    for k in 0..hidden {
                                        pooled[k] += data[j * hidden + k];
                                    }
                                    count += 1;
                                }
                            }
                            if count > 0 {
                                self.dim = hidden; // 校准真实维度，后续 encode 复用
                                for val in &mut pooled {
                                    *val /= count as f32;
                                }
                                return Ok(normalize_l2(&pooled));
                            }
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
