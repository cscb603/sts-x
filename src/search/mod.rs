/*
 * search/mod.rs
 * Project: sts-x
 * Description: Search pipeline orchestrator
 *
 * Default: BM25 only, zero heavy deps.
 * With `--features semantic`: optional embedding + BGE reranker.
 */

use crate::embed::EmbeddingModel;
use crate::indexer::SearchIndex;
use crate::types::*;
use anyhow::Result;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// Search engine orchestrator
pub struct SearchEngine {
    index: Arc<SearchIndex>,
    embed_model: Option<EmbeddingModel>,
    #[cfg(feature = "semantic")]
    reranker: Option<Reranker>,
}

impl SearchEngine {
    pub fn new(index: Arc<SearchIndex>, embed_model: Option<EmbeddingModel>) -> Self {
        Self {
            index,
            embed_model,
            #[cfg(feature = "semantic")]
            reranker: None,
        }
    }

    #[cfg(feature = "semantic")]
    pub fn with_reranker(mut self, reranker: Reranker) -> Self {
        self.reranker = Some(reranker);
        self
    }

    /// Execute a search query (dispatches by mode + output mode)
    pub fn search(&mut self, query: SearchQuery) -> Result<SearchResponse> {
        match query.mode {
            SearchMode::Filename => self.search_filename_mode(&query),
            SearchMode::All => self.search_all_mode(&query),
            SearchMode::Code => {
                // None (unspecified) defaults to Expand; entry points resolve
                // auto-routing before reaching the engine (see src/router.rs).
                if matches!(query.output_mode, Some(crate::types::OutputMode::Locate)) {
                    self.search_code_locate(&query)
                } else {
                    self.search_code_mode(&query)
                }
            }
        }
    }

    /// Locate mode (3.0): grep-sized line hits inside the top AST blocks.
    /// Returns individual matching lines (with small context) instead of whole blocks,
    /// so the AI gets the location cheaply (~130 tok) before deciding to `--expand`.
    fn search_code_locate(&self, query: &SearchQuery) -> Result<SearchResponse> {
        let start = Instant::now();

        // Match terms. For a single long token with no whitespace (e.g.
        // `select_best_cfg`), treat the whole query as one term so it still matches.
        let mut terms: Vec<String> = query
            .query
            .split_whitespace()
            .filter(|t| t.len() >= 2)
            .map(|t| t.to_lowercase())
            .collect();
        if terms.is_empty() {
            terms.push(query.query.to_lowercase());
        }

        // Grep-sized budget: at most 1-2 hits TOTAL so locate stays ~130-200 tok
        // even for long (CJK) contexts. This cap applies to BOTH the BM25 path and
        // the live-grep fallback — the old code only capped BM25 and let an uncapped
        // fallback dump many `.md` hits (→ 502 tok for `select_best_cfg`).
        let budget = query.top_k.clamp(1, 2);

        // Keep the file path short (last 3 components) so locate stays token-cheap
        // even inside deeply-nested dirs. The AI expands by symbol if it needs more.
        let short_path = |p: &str| -> String {
            let comps: Vec<&str> = p.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
            if comps.len() > 3 {
                comps[comps.len() - 3..].join("/")
            } else {
                p.to_string()
            }
        };

        let mut matches: Vec<LocateMatch> = Vec::new();
        let mut seen_paths: HashSet<String> = HashSet::new();

        // ── Path A: BM25 over AST chunks (fast, ranked) ──────────────
        let raw =
            self.index
                .search_text(&query.query, query.top_k * 3, query.path_filter.as_deref())?;
        for (score, ib) in raw.iter() {
            if matches.len() >= budget {
                break;
            }
            let lines: Vec<&str> = ib.block.code.lines().collect();
            // Definition-line priority (whitepaper §7 P0-2): the signature line
            // (`fn foo(` / `pub struct X {`) is line 0 of most AST blocks —
            // prefer it over a body line so the AI lands ON the definition,
            // not on a doc comment / inner call.
            let hit_off = if !lines.is_empty() {
                let sig_hit = {
                    let low = lines[0].to_lowercase();
                    terms.iter().any(|t| low.contains(t))
                };
                if sig_hit {
                    Some(0)
                } else {
                    lines
                        .iter()
                        .enumerate()
                        .find(|(_, line)| {
                            let low = line.to_lowercase();
                            terms.iter().any(|t| low.contains(t))
                        })
                        .map(|(off, _)| off)
                }
            } else {
                None
            };
            if let Some(off) = hit_off {
                let abs_line = ib.block.start_line + off;
                let trimmed = lines[off].trim();
                let ctx: String = if trimmed.chars().count() > 48 {
                    format!("{}…", trimmed.chars().take(48).collect::<String>())
                } else {
                    trimmed.to_string()
                };
                let path_str = ib.block.path.display().to_string();
                seen_paths.insert(path_str.clone());
                matches.push(LocateMatch {
                    score: if *score > 0.0 { (*score).min(1.0) } else { 0.0 },
                    file: short_path(&path_str),
                    abs_path: ib.block.abs_path.display().to_string(),
                    line: abs_line,
                    context: ctx,
                    kind: format!("{:?}", ib.block.kind).to_lowercase(),
                    name: ib.block.name.clone(),
                });
            }
        }

        // ── Path B: live grep over CODE files (gitignore-aware, binary-skipping),
        //    capped at the REMAINING budget. Safety net for queries whose best hit
        //    is in a code file the chunker missed, or an unindexed path. Code files
        //    (not .md docs) are searched here, and the cap is always respected. ──
        if matches.len() < budget {
            let mut live = self.index.search_code_live(
                &terms,
                budget - matches.len(),
                &seen_paths,
                query.path_filter.as_deref(),
            )?;
            for m in live.iter_mut() {
                m.file = short_path(&m.file);
            }
            matches.append(&mut live);
        }

        let elapsed = start.elapsed().as_millis() as u64;
        // v5.1 (P0-2): zero-hit locate → rescue hint so the AI can self-recover.
        // v5.1-3: build_hint returns Option (None on hits>0); called with 0 here.
        let hint = if matches.is_empty() {
            crate::types::format::build_hint(&query.query, 0, "locate")
        } else {
            None
        };
        Ok(SearchResponse {
            query: query.query.clone(),
            total_hits: matches.len(),
            results: Vec::new(),
            search_time_ms: elapsed,
            multi_hop: None,
            locate_matches: matches,
            hint,
        })
    }

    /// Code search (AST chunks, BM25 + optional embedding)
    fn search_code_mode(&mut self, query: &SearchQuery) -> Result<SearchResponse> {
        let start = Instant::now();

        // ── R2: symbol fast path ─────────────────────────────────────
        // A symbol-like query (e.g. `run_search`, `cache::detect_project_root`)
        // whose exact chunk name exists in the index returns those blocks
        // directly — focused results instead of BM25 term-scatter. Queries
        // without an exact name hit fall through to the hybrid path below.
        if crate::router::is_symbol(&query.query) {
            let q = query.query.trim();
            // `mod::path::name` → compare against the trailing segment
            let needle = q.rsplit("::").next().unwrap_or(q);
            let mut exact: Vec<SearchResult> = self
                .index
                .all_blocks()
                .iter()
                .filter(|ib| ib.block.name == needle)
                .map(|ib| SearchResult {
                    score: 1.0,
                    block: ib.block.clone(),
                    highlight_lines: Vec::new(),
                    explanation: "exact symbol match".to_string(),
                })
                .collect();
            if !exact.is_empty() {
                exact.truncate(query.top_k.max(1));
                truncate_by_tokens(&mut exact, query.max_tokens);
                let elapsed = start.elapsed().as_millis() as u64;
                return Ok(SearchResponse {
                    query: query.query.clone(),
                    total_hits: exact.len(),
                    results: exact,
                    search_time_ms: elapsed,
                    multi_hop: None,
                    locate_matches: Vec::new(),
                    hint: None,
                });
            }
        }

        let query_embedding = self.embed_model.as_mut().and_then(|m| {
            let query_text = format!("query: {}", query.query);
            m.encode(&query_text).ok()
        });

        let raw_results = self.index.search_hybrid(
            &query.query,
            query_embedding.as_deref(),
            query.top_k * 3,
            query.path_filter.as_deref(),
        )?;

        #[cfg(feature = "semantic")]
        let results = if let Some(ref mut reranker) = self.reranker {
            reranker.rerank(&query.query, &raw_results, query.top_k)?
        } else {
            normalize_scores(&raw_results, query.top_k)
        };

        #[cfg(not(feature = "semantic"))]
        let results = normalize_scores(&raw_results, query.top_k);

        let mut results = results;

        // ── P0-2: 0 命中 → 向量召回（治本跨语言语义鸿沟）────────────────
        // BM25 单独 0 命中的中文 NL 查询，若 semantic 已启用（模型可用），
        // 用查询 embedding 走 cosine 召回，并入 results。向量检索不依赖
        // 字面 token，中文描述也能命中语义相近的英文代码块。
        if results.is_empty() {
            if let Some(qe) = query_embedding.as_deref() {
                match self.index.search_vector(qe, query.top_k * 2) {
                    Ok(vec_raw) if !vec_raw.is_empty() => {
                        tracing::info!(
                            "vector recall (semantic fallback): query=\"{}\" hits={}",
                            query.query,
                            vec_raw.len()
                        );
                        let mut vec_results = normalize_scores(&vec_raw, query.top_k);
                        for r in vec_results.iter_mut() {
                            if r.explanation.is_empty() {
                                r.explanation = "vector recall (semantic fallback)".to_string();
                            }
                        }
                        results = vec_results;
                    }
                    _ => {}
                }
            }
        }

        // ── P0-1: 仍 0 命中 → 自动重试链（本地词典/符号/文件兜底）──────
        // BM25 与向量都不命中时，按固定顺序自动重试：英文同义词展开 →
        // 符号猜测 → file 内容兜底。单次调用即出最佳结果，AI 无需按 hint
        // 手动换词再调（消除中文弱时的多往返死循环）。
        if results.is_empty() {
            results = self.auto_retry(query)?;
        }

        // Apply max_tokens budget truncation
        truncate_by_tokens(&mut results, query.max_tokens);

        let elapsed = start.elapsed().as_millis() as u64;

        Ok(SearchResponse {
            query: query.query.clone(),
            total_hits: results.len(),
            results,
            search_time_ms: elapsed,
            multi_hop: None,
            locate_matches: Vec::new(),
            hint: None,
        })
    }

    /// P0-1: 0 命中自动重试链 — 按 `rewrite::retry_plan` 的固定顺序重试：
    ///   1. EnglishSynonym：中英词典映射的英文词 BM25（如 缓存→cache）；
    ///   2. SymbolGuess：去停用词后抽 ASCII 标识符 BM25（如 "hint"）；
    ///   3. FileFallback：`sts-x file` 文件名/内容兜底（rg / ignore walker）。
    ///
    /// 任一阶段命中即合并返回（命中即停，避免低质兜底污染 top_k）；
    /// 全部不命中才返回空，由外层 `build_hint` 生成最终自救文本。
    /// 重试结果的 `explanation` 标注来源，AI 可区分正常命中与重试命中。
    fn auto_retry(&self, query: &SearchQuery) -> Result<Vec<SearchResult>> {
        let plan = crate::rewrite::retry_plan(&query.query);
        let top_k = query.top_k.max(2);
        let pf = query
            .path_filter
            .as_deref()
            .map(|p| p.trim())
            .filter(|p| !p.is_empty());
        let mut merged: Vec<SearchResult> = Vec::new();

        for (kind, q) in plan {
            if !merged.is_empty() {
                break; // 已命中 → 不再降级到更弱的重试阶段
            }
            match kind {
                crate::rewrite::RetryKind::EnglishSynonym
                | crate::rewrite::RetryKind::SymbolGuess => {
                    let raw =
                        self.index
                            .search_text(&q, top_k * 2, query.path_filter.as_deref())?;
                    let mut rs = normalize_scores(&raw, top_k);
                    tracing::info!(
                        "auto_retry({}): query=\"{}\" hits={}",
                        kind.label(),
                        q,
                        rs.len()
                    );
                    for r in rs.iter_mut() {
                        if r.explanation.is_empty() {
                            r.explanation = format!("auto-retry({}): {}", kind.label(), q);
                        }
                    }
                    merged.append(&mut rs);
                }
                crate::rewrite::RetryKind::FileFallback => {
                    let cfg = self.index.config();
                    let matches =
                        crate::filesearch::search_files(&q, &cfg.project_root, false, top_k, true)?;
                    tracing::info!(
                        "auto_retry(file-fallback): query=\"{}\" hits={}",
                        q,
                        matches.len()
                    );
                    for m in matches {
                        if let Some(pf) = pf {
                            if !m.path.contains(pf) {
                                continue; // P0-3: file 兜底同样尊重 --path-filter
                            }
                        }
                        let path = std::path::PathBuf::from(&m.path);
                        let name = path
                            .file_stem()
                            .map(|s| s.to_string_lossy().to_string())
                            .unwrap_or_default();
                        merged.push(SearchResult {
                            score: 0.75,
                            block: CodeBlock {
                                path,
                                abs_path: std::path::PathBuf::from(&m.abs_path),
                                kind: crate::types::BlockKind::Block,
                                name,
                                signature: String::new(),
                                doc_comment: String::new(),
                                code: m.context,
                                language: String::new(),
                                start_line: if m.line > 0 { m.line } else { 1 },
                                end_line: if m.line > 0 { m.line } else { 1 },
                                imports: Vec::new(),
                            },
                            highlight_lines: if m.line > 0 { vec![m.line] } else { Vec::new() },
                            explanation: format!("auto-retry(file-fallback): {}", m.matched_by),
                        });
                        if merged.len() >= top_k {
                            break;
                        }
                    }
                }
            }
        }

        Ok(merged)
    }

    /// Filename search (live walk, substring match)
    fn search_filename_mode(&self, query: &SearchQuery) -> Result<SearchResponse> {
        let start = Instant::now();
        let config = self.index.config();
        let mut results = SearchIndex::search_filename_live(&query.query, config, query.top_k)?;
        truncate_by_tokens(&mut results, query.max_tokens);
        let elapsed = start.elapsed().as_millis() as u64;

        Ok(SearchResponse {
            query: query.query.clone(),
            total_hits: results.len(),
            results,
            search_time_ms: elapsed,
            multi_hop: None,
            locate_matches: Vec::new(),
            hint: None,
        })
    }

    /// All-files search (code + non-code, filename + content)
    fn search_all_mode(&self, query: &SearchQuery) -> Result<SearchResponse> {
        let start = Instant::now();

        // Step 1: Get code search results from index
        let code_results =
            self.index
                .search_text(&query.query, query.top_k * 2, query.path_filter.as_deref())?;
        let code_results = normalize_scores(&code_results, query.top_k);

        // Step 2: Live grep non-code files
        let config = self.index.config();
        let file_results = self.index.search_all_files(
            &query.query,
            config,
            query.top_k,
            query.path_filter.as_deref(),
        )?;

        // Step 3: Merge — code results first, then file results
        let mut merged = Vec::new();
        for r in code_results {
            merged.push(r);
        }
        for r in file_results {
            // Skip duplicates by path
            if !merged.iter().any(|m| m.block.path == r.block.path) {
                merged.push(r);
            }
        }
        merged.truncate(query.top_k);
        truncate_by_tokens(&mut merged, query.max_tokens);

        // Step 4: Build locate_matches for --all --locate support
        // (search_all_files returns SearchResult, but --locate reads locate_matches)
        let locate_matches: Vec<LocateMatch> = merged
            .iter()
            .filter_map(|r| {
                let block = &r.block;
                if block.code.is_empty() && block.start_line == 0 {
                    // filename match — no specific line
                    None
                } else {
                    Some(LocateMatch {
                        score: r.score,
                        file: block.path.display().to_string(),
                        abs_path: block.abs_path.display().to_string(),
                        line: if block.start_line > 0 {
                            block.start_line
                        } else {
                            1
                        },
                        context: block
                            .code
                            .lines()
                            .next()
                            .unwrap_or("")
                            .chars()
                            .take(48)
                            .collect(),
                        kind: block.language.clone(),
                        name: block.name.clone(),
                    })
                }
            })
            .collect();

        let elapsed = start.elapsed().as_millis() as u64;

        Ok(SearchResponse {
            query: query.query.clone(),
            total_hits: merged.len(),
            results: merged,
            search_time_ms: elapsed,
            multi_hop: None,
            locate_matches,
            hint: None,
        })
    }
}

/// Estimate token count from character count (fast budget, no model inference).
/// v5.1-3: CJK chars weighted 1.5 (a Chinese char ≈ 1-1.5 tokens in mixed
/// code+Chinese content; the old flat 0.5/char estimate was too optimistic and
/// silently overshot the budget on Chinese projects). ASCII stays 0.5/char.
fn estimate_tokens(text: &str) -> usize {
    let mut cjk = 0usize;
    let mut ascii = 0usize;
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            cjk += 1;
        }
    }
    // ASCII ≈ 2 chars/token, CJK ≈ 1.5 chars/token → (ascii+1)/2 + (cjk*2)/3
    ascii.div_ceil(2) + (cjk * 2).div_ceil(3)
}

/// Truncate results to fit within `max_tokens` budget.
/// Drops lowest-score results until the estimated total is within budget.
/// Each result's token estimate is based on its code + signature content.
fn truncate_by_tokens(results: &mut Vec<SearchResult>, max_tokens: usize) {
    if max_tokens == 0 || results.is_empty() {
        return;
    }
    // Sort by score descending first (should already be sorted)
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut total: usize = 0;
    let mut keep: Vec<SearchResult> = Vec::new();
    for r in results.drain(..) {
        let content = format!(
            "{}\n{}\n{}\n{}",
            r.block.name, r.block.signature, r.block.code, r.block.doc_comment
        );
        let tok = estimate_tokens(&content);
        if total + tok <= max_tokens || keep.is_empty() {
            total += tok;
            keep.push(r);
        } else {
            // This result would exceed the budget; drop it
            break;
        }
    }
    *results = keep;
}

/// Normalize BM25 scores to 0-1 range and take top_k.
/// P1-1: recency boost — files modified ≤1 day ago get +0.15, ≤7 days +0.10,
/// so freshly-touched code surfaces above stale hits of similar relevance.
fn normalize_scores(
    raw: &[(f32, &crate::indexer::IndexedBlock)],
    top_k: usize,
) -> Vec<SearchResult> {
    let max_score = raw.first().map(|(s, _)| *s).unwrap_or(1.0);
    let now = std::time::SystemTime::now();
    raw.iter()
        .take(top_k)
        .map(|(score, ib)| {
            let mut norm_score = if max_score > 0.0 {
                score / max_score
            } else {
                0.0
            };
            // Recency boost from the file's mtime (read live from disk — the
            // whitepaper's manifest.json does not exist in this codebase).
            if let Ok(meta) = std::fs::metadata(&ib.block.abs_path) {
                if let Ok(modified) = meta.modified() {
                    if let Ok(age) = now.duration_since(modified) {
                        let secs = age.as_secs();
                        if secs <= 86_400 {
                            norm_score += 0.15;
                        } else if secs <= 7 * 86_400 {
                            norm_score += 0.10;
                        }
                    }
                }
            }
            SearchResult {
                score: norm_score,
                block: ib.block.clone(),
                highlight_lines: Vec::new(),
                explanation: String::new(),
            }
        })
        .collect()
}

// ─── BGE Reranker (only with semantic feature) ─────────────────────

#[cfg(feature = "semantic")]
mod reranker {
    use crate::indexer::IndexedBlock;
    use crate::types::*;
    use anyhow::Result;
    use ort::value::Value as OrtValue;
    use std::path::Path;
    use tokenizers::Tokenizer;

    pub struct Reranker {
        session: ort::session::Session,
        tokenizer: Tokenizer,
        max_length: usize,
    }

    impl Reranker {
        pub fn load(model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
            let session = ort::session::Session::builder()?.commit_from_file(model_path)?;
            let tokenizer = Tokenizer::from_file(tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load reranker tokenizer: {}", e))?;
            Ok(Self {
                session,
                tokenizer,
                max_length: 512,
            })
        }

        pub fn rerank(
            &mut self,
            query: &str,
            candidates: &[(f32, &IndexedBlock)],
            top_k: usize,
        ) -> Result<Vec<SearchResult>> {
            let mut scored: Vec<(f32, &IndexedBlock)> = Vec::new();
            for (_, ib) in candidates.iter() {
                let text = format!("{} [SEP] {}", query, ib.block.signature);
                let score = self.score_pair(&text, &ib.block.code)?;
                scored.push((score, *ib));
            }

            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

            let max_score = scored.first().map(|(s, _)| *s).unwrap_or(1.0);
            Ok(scored
                .iter()
                .take(top_k)
                .map(|(score, ib)| SearchResult {
                    score: if max_score > 0.0 {
                        *score / max_score
                    } else {
                        0.0
                    },
                    block: ib.block.clone(),
                    highlight_lines: Vec::new(),
                    explanation: format!("reranker: {:.4}", score),
                })
                .collect())
        }

        fn score_pair(&mut self, query: &str, doc: &str) -> Result<f32> {
            use ort::inputs;

            let text = format!("{} [SEP] {}", query, doc);
            let encoding = self
                .tokenizer
                .encode(text, true)
                .map_err(|e| anyhow::anyhow!("Reranker tokenization failed: {}", e))?;

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

            let input_tensor = OrtValue::from_array(([1usize, padded_len], padded_ids))?;
            let mask_tensor = OrtValue::from_array(([1usize, padded_len], padded_mask))?;
            let type_tensor = OrtValue::from_array(([1usize, padded_len], padded_types))?;

            let outputs = self.session.run(inputs!(
                "input_ids" => input_tensor,
                "attention_mask" => mask_tensor,
                "token_type_ids" => type_tensor,
            ))?;

            let mut score = 0.0f32;
            for output in outputs.iter() {
                if let Ok((_shape, data)) = output.1.try_extract_tensor::<f32>() {
                    score = *data.iter().next().unwrap_or(&0.0);
                    break;
                }
            }

            Ok(sigmoid(score))
        }
    }

    fn sigmoid(x: f32) -> f32 {
        1.0 / (1.0 + (-x).exp())
    }
}

#[cfg(feature = "semantic")]
pub use reranker::Reranker;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{BlockKind, CodeBlock, IndexConfig, SearchMode};

    /// 唯一临时目录（并行测试避免争抢 Tantivy 写锁）。
    fn temp_config(tag: &str) -> IndexConfig {
        let root = std::env::temp_dir().join(format!("stsx-retry-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        IndexConfig {
            project_root: root.clone(),
            index_path: root.join("idx"),
            ..Default::default()
        }
    }

    fn block(path: &str, name: &str, code: &str) -> CodeBlock {
        CodeBlock {
            path: std::path::PathBuf::from(path),
            abs_path: std::path::PathBuf::from(path),
            kind: BlockKind::Function,
            name: name.to_string(),
            signature: format!("fn {}() {{", name),
            doc_comment: String::new(),
            code: code.to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 1 + code.lines().count(),
            imports: Vec::new(),
        }
    }

    fn engine_with_blocks(cfg: &IndexConfig, blocks: Vec<CodeBlock>) -> SearchEngine {
        let mut idx = crate::indexer::SearchIndex::new(cfg.clone(), None).unwrap();
        idx.index_blocks(blocks).unwrap();
        SearchEngine::new(std::sync::Arc::new(idx), None)
    }

    fn expand_query(q: &str) -> SearchQuery {
        SearchQuery {
            query: q.to_string(),
            mode: SearchMode::Code,
            output_mode: Some(crate::types::OutputMode::Expand),
            top_k: 2,
            ..Default::default()
        }
    }

    /// P0-1 验收 1：中文 0 命中 → 自动英文同义词展开 → 命中英文代码块。
    /// 代码库无任何中文，中文查询 BM25 必然 0 命中；"提示"→hint 词典映射后
    /// 命中 build_hint，不再纯 0 命中甩 hint。
    #[test]
    fn zero_hit_chinese_auto_retries_to_english_synonym() {
        let cfg = temp_config("syn");
        let mut engine = engine_with_blocks(
            &cfg,
            vec![block(
                "src/format.rs",
                "build_hint",
                "pub fn build_hint() { println!(\"rescue guidance\"); }",
            )],
        );
        let resp = engine.search(expand_query("提示")).unwrap();
        assert!(!resp.results.is_empty(), "auto-retry should hit, got empty");
        assert!(
            resp.results
                .iter()
                .any(|r| r.explanation.contains("auto-retry")),
            "explanations: {:?}",
            resp.results
                .iter()
                .map(|r| r.explanation.clone())
                .collect::<Vec<_>>()
        );
        assert!(
            resp.results
                .iter()
                .any(|r| r.block.name.contains("build_hint")),
            "should have recovered build_hint via english synonym"
        );
    }

    /// P0-1 验收 2：有命中时不触发重试（无 "auto-retry" 污染）。
    #[test]
    fn hit_queries_do_not_trigger_retry() {
        let cfg = temp_config("hit");
        let mut engine = engine_with_blocks(
            &cfg,
            vec![block(
                "src/cache.rs",
                "cache_helper",
                "fn cache_helper() { let x = 42; }",
            )],
        );
        let resp = engine.search(expand_query("cache")).unwrap();
        assert!(!resp.results.is_empty(), "cache should hit directly");
        assert!(
            resp.results
                .iter()
                .all(|r| !r.explanation.contains("auto-retry")),
            "no auto-retry expected on hits: {:?}",
            resp.results
                .iter()
                .map(|r| r.explanation.clone())
                .collect::<Vec<_>>()
        );
    }

    /// P0-1 验收 3：BM25/词典/符号全不中 → file 兜底（文件名/内容子串）。
    #[test]
    fn zero_hit_falls_back_to_file_content() {
        let cfg = temp_config("file");
        // 项目根下放一个中文文档文件；代码块无 hint/fallback/guide 字样
        std::fs::create_dir_all(cfg.project_root.join("docs")).unwrap();
        std::fs::write(
            cfg.project_root.join("docs/中文文档.txt"),
            "自救攻略在这里\n",
        )
        .unwrap();
        let mut engine = engine_with_blocks(
            &cfg,
            vec![block(
                "src/cache.rs",
                "cache_helper",
                "fn cache_helper() { let x = 42; }",
            )],
        );
        let resp = engine.search(expand_query("自救攻略")).unwrap();
        assert!(
            !resp.results.is_empty(),
            "file fallback should hit docs/中文文档.txt"
        );
        assert!(
            resp.results
                .iter()
                .any(|r| r.explanation.contains("file-fallback")),
            "explanations: {:?}",
            resp.results
                .iter()
                .map(|r| r.explanation.clone())
                .collect::<Vec<_>>()
        );
        assert!(
            resp.results
                .iter()
                .any(|r| r.block.path.display().to_string().contains("中文文档")),
            "should hit the chinese doc file"
        );
    }

    /// P0-1 兼容：重试全部失败 → results 空 + build_hint 仍给自救文本（旧客户端契约）。
    #[test]
    fn zero_hit_after_all_retries_still_emits_hint() {
        let cfg = temp_config("miss");
        let mut engine = engine_with_blocks(
            &cfg,
            vec![block(
                "src/cache.rs",
                "cache_helper",
                "fn cache_helper() { let x = 42; }",
            )],
        );
        let q = "量子纠缠不存在的检索目标";
        let resp = engine.search(expand_query(q)).unwrap();
        assert!(
            resp.results.is_empty(),
            "nothing should match: {:?}",
            resp.results
        );
        let hint = crate::types::format::build_hint(q, 0, "expand");
        assert!(hint.is_some(), "0-hit contract: hint must be emitted");
        let h = hint.unwrap();
        assert!(h.contains("换英文关键词"), "hint={h}");
    }
}
