/*
 * search/mod.rs
 * Project: sts-x
 * Description: Search pipeline orchestrator
 *
 * Default: BM25 only, zero heavy deps.
 * With `--features semantic`: optional embedding + BGE reranker.
 */

use crate::types::*;
use crate::indexer::SearchIndex;
use crate::embed::EmbeddingModel;
use anyhow::Result;
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

    /// Execute a search query (dispatches by mode)
    pub fn search(&mut self, query: SearchQuery) -> Result<SearchResponse> {
        match query.mode {
            SearchMode::Filename => self.search_filename_mode(&query),
            SearchMode::All => self.search_all_mode(&query),
            SearchMode::Code => self.search_code_mode(&query),
        }
    }

    /// Code search (AST chunks, BM25 + optional embedding)
    fn search_code_mode(&mut self, query: &SearchQuery) -> Result<SearchResponse> {
        let start = Instant::now();

        let query_embedding = self.embed_model.as_mut().and_then(|m| {
            let query_text = format!("query: {}", query.query);
            m.encode(&query_text).ok()
        });

        let raw_results = self.index.search_hybrid(
            &query.query,
            query_embedding.as_deref(),
            query.top_k * 3,
        )?;

        #[cfg(feature = "semantic")]
        let results = if let Some(ref mut reranker) = self.reranker {
            reranker.rerank(&query.query, &raw_results, query.top_k)?
        } else {
            normalize_scores(&raw_results, query.top_k)
        };

        #[cfg(not(feature = "semantic"))]
        let results = normalize_scores(&raw_results, query.top_k);

        let elapsed = start.elapsed().as_millis() as u64;

        Ok(SearchResponse {
            query: query.query.clone(),
            total_hits: results.len(),
            results,
            search_time_ms: elapsed,
            multi_hop: None,
        })
    }

    /// Filename search (live walk, substring match)
    fn search_filename_mode(&self, query: &SearchQuery) -> Result<SearchResponse> {
        let start = Instant::now();
        let config = self.index.config();
        let results = SearchIndex::search_filename_live(&query.query, config, query.top_k)?;
        let elapsed = start.elapsed().as_millis() as u64;

        Ok(SearchResponse {
            query: query.query.clone(),
            total_hits: results.len(),
            results,
            search_time_ms: elapsed,
            multi_hop: None,
        })
    }

    /// All-files search (code + non-code, filename + content)
    fn search_all_mode(&self, query: &SearchQuery) -> Result<SearchResponse> {
        let start = Instant::now();

        // Step 1: Get code search results from index
        let code_results = self.index.search_text(&query.query, query.top_k * 2)?;
        let code_results = normalize_scores(&code_results, query.top_k);

        // Step 2: Live grep non-code files
        let config = self.index.config();
        let file_results = self.index.search_all_files(&query.query, config, query.top_k)?;

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

        let elapsed = start.elapsed().as_millis() as u64;

        Ok(SearchResponse {
            query: query.query.clone(),
            total_hits: merged.len(),
            results: merged,
            search_time_ms: elapsed,
            multi_hop: None,
        })
    }
}

/// Normalize BM25 scores to 0-1 range and take top_k
fn normalize_scores(raw: &[(f32, &crate::indexer::IndexedBlock)], top_k: usize) -> Vec<SearchResult> {
    let max_score = raw.first().map(|(s, _)| *s).unwrap_or(1.0);
    raw.iter()
        .take(top_k)
        .map(|(score, ib)| {
            let norm_score = if max_score > 0.0 { score / max_score } else { 0.0 };
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
    use crate::types::*;
    use crate::indexer::IndexedBlock;
    use anyhow::Result;
    use std::path::Path;
    use tokenizers::Tokenizer;
    use ort::value::Value as OrtValue;

    pub struct Reranker {
        session: ort::session::Session,
        tokenizer: Tokenizer,
        max_length: usize,
    }

    impl Reranker {
        pub fn load(model_path: &Path, tokenizer_path: &Path) -> Result<Self> {
            let session = ort::session::Session::builder()?
                .commit_from_file(model_path)?;
            let tokenizer = Tokenizer::from_file(tokenizer_path)
                .map_err(|e| anyhow::anyhow!("Failed to load reranker tokenizer: {}", e))?;
            Ok(Self { session, tokenizer, max_length: 512 })
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
            Ok(scored.iter().take(top_k).map(|(score, ib)| SearchResult {
                score: if max_score > 0.0 { *score / max_score } else { 0.0 },
                block: ib.block.clone(),
                highlight_lines: Vec::new(),
                explanation: format!("reranker: {:.4}", score),
            }).collect())
        }

        fn score_pair(&mut self, query: &str, doc: &str) -> Result<f32> {
            use ort::inputs;

            let text = format!("{} [SEP] {}", query, doc);
            let encoding = self.tokenizer.encode(text, true)
                .map_err(|e| anyhow::anyhow!("Reranker tokenization failed: {}", e))?;

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
