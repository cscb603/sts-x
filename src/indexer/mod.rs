/*
 * indexer/mod.rs
 * Project: sts-x
 * Description: Full-text and vector indexing engine
 *
 * Dual index architecture:
 * 1. Tantivy (BM25) — fast keyword/full-text search
 * 2. Flat-vector — in-memory cosine similarity search
 *
 * The flat-vector approach avoids external dependencies (no Qdrant server needed)
 * and is sufficient for project-level code search (10K-100K code blocks).
 * For larger scale, a dedicated vector DB can be swapped in.
 */

use crate::types::{CodeBlock, IndexConfig, LocateMatch, SearchResult};
use std::collections::HashSet;
use crate::embed::EmbeddingModel;
use anyhow::{Context, Result};
use std::collections::HashMap;
use serde::{Serialize, Deserialize};
use tantivy::{
    doc,
    query::{TermQuery, BooleanQuery, Occur, Query},
    schema::*,
    tokenizer::{TextAnalyzer, SimpleTokenizer, LowerCaser, RemoveLongFilter},
    Term,
    IndexWriter, IndexReader, Index, ReloadPolicy,
    TantivyDocument,
};

/// A fully indexed code block with its embedding vector
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedBlock {
    pub block: CodeBlock,
    pub embedding: Vec<f32>,
}

/// Combined search index (full-text + vector)
pub struct SearchIndex {
    /// Tantivy text index (BM25)
    text_index: Index,
    text_reader: IndexReader,
    schema: Schema,
    /// In-memory vector store: indexed blocks with embeddings
    vector_store: Vec<IndexedBlock>,
    /// Mapping from tantivy doc_id to vector_store index
    doc_id_to_vector: HashMap<u32, usize>,
    /// Embedding model
    embed_model: Option<EmbeddingModel>,
    /// Config
    config: IndexConfig,
}

/// Tantivy schema fields
const FIELD_PATH: &str = "path";
const FIELD_NAME: &str = "name";
const FIELD_SIGNATURE: &str = "signature";
const FIELD_CODE: &str = "code";
const FIELD_DOC_COMMENT: &str = "doc_comment";
const FIELD_LANGUAGE: &str = "language";
const FIELD_KIND: &str = "kind";
const FIELD_START_LINE: &str = "start_line";
const FIELD_END_LINE: &str = "end_line";

/// File extensions treated as code (AST-indexed)
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "jsx", "mjs", "ts", "tsx", "go", "java", "c", "cpp", "cc", "cxx", "hpp",
    "php", "rb", "swift", "scala", "sc",
];

/// Binary/text extensions to skip entirely (don't even index path)
const SKIP_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "bmp", "ico", "icns",
    "mp3", "mp4", "avi", "mov", "wav", "flac",
    "zip", "tar", "gz", "bz2", "7z", "rar",
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
    "ttf", "otf", "woff", "woff2", "eot",
    "o", "so", "dylib", "dll", "exe", "dmg", "app",
    "wasm", "rlib", "rmeta",
];

/// Noise directory/file patterns — paths containing any of these are skipped.
/// Used to filter out backup copies, old versions, and other junk files.
const NOISE_PATTERNS: &[&str] = &[
    "_backup", "_original", "_old", "_copy", "复制", "副本",
    ".bak", ".swp", ".tmp",
];

/// Check if a relative path string contains any noise pattern.
/// Used in all walk/file-scanning functions to skip backups/copies.
fn is_noise_path(rel_str: &str) -> bool {
    NOISE_PATTERNS.iter().any(|p| rel_str.contains(p))
}

impl SearchIndex {
    /// Create a new empty search index
    pub fn new(config: IndexConfig, embed_model: Option<EmbeddingModel>) -> Result<Self> {
        let code_tokenizer = TextAnalyzer::builder(SimpleTokenizer::default())
            .filter(RemoveLongFilter::limit(128))
            .filter(LowerCaser)
            .build();

        let mut schema_builder = Schema::builder();
        schema_builder.add_text_field(FIELD_PATH, STRING | STORED);
        schema_builder.add_text_field(FIELD_NAME, TEXT | STORED);
        schema_builder.add_text_field(FIELD_SIGNATURE, TEXT | STORED);
        let code_options = TextOptions::default().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer("code")
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        ).set_stored();
        schema_builder.add_text_field(FIELD_CODE, code_options.clone());
        schema_builder.add_text_field(FIELD_DOC_COMMENT, TEXT | STORED);
        schema_builder.add_text_field(FIELD_LANGUAGE, STRING | STORED);
        schema_builder.add_text_field(FIELD_KIND, STRING | STORED);
        schema_builder.add_u64_field(FIELD_START_LINE, STORED);
        schema_builder.add_u64_field(FIELD_END_LINE, STORED);
        let schema = schema_builder.build();

        let index_path = config.index_path.join("tantivy");
        std::fs::create_dir_all(&index_path).ok();

        let text_index = if index_path.join("meta.json").exists() {
            let idx = Index::open_in_dir(&index_path)
                .context("Failed to open existing tantivy index")?;
            idx.tokenizers().register("code", code_tokenizer);
            idx
        } else {
            let idx = Index::create_in_dir(&index_path, schema.clone())
                .context("Failed to create tantivy index")?;
            idx.tokenizers().register("code", code_tokenizer);
            idx
        };

        let text_reader = text_index
        .reader_builder()
        .reload_policy(ReloadPolicy::OnCommitWithDelay)
        .try_into()?;

    let mut index = Self {
        text_index,
        text_reader,
        schema,
        vector_store: Vec::new(),
        doc_id_to_vector: HashMap::new(),
        embed_model,
        config,
    };

    // If loading an existing index, populate vector_store from stored docs
    if index_path.join("meta.json").exists() {
        index.load_vector_store_from_tantivy()?;
    }

    Ok(index)
}

    /// Index a batch of code blocks
    pub fn index_blocks(&mut self, blocks: Vec<CodeBlock>) -> Result<()> {
        let mut writer: IndexWriter<TantivyDocument> = self.text_index.writer(50_000_000)?;

        for block in &blocks {
            // Add to tantivy
            let doc = self.create_tantivy_doc(block);
            writer.add_document(doc)?;
        }

        writer.commit()?;
        self.text_reader.reload()?;

        // Generate embeddings and add to vector store
        if let Some(ref mut model) = self.embed_model {
            let texts: Vec<String> = blocks
                .iter()
                .map(|b| format!("{}\n{}", b.signature, b.code))
                .collect();

            let embeddings = model.encode_batch(&texts)?;

            for (i, (block, embedding)) in blocks.into_iter().zip(embeddings).enumerate() {
                let vidx = self.vector_store.len();
                self.vector_store.push(IndexedBlock { block, embedding });
                self.doc_id_to_vector.insert(i as u32, vidx);
            }
        } else {
            for block in blocks {
                let vidx = self.vector_store.len();
                self.vector_store.push(IndexedBlock {
                    block,
                    embedding: Vec::new(),
                });
                // Map dummy doc_id
                self.doc_id_to_vector.insert(vidx as u32, vidx);
            }
        }

        Ok(())
    }

    /// Index all file paths in the project (for filename search)
    /// Non-code files get a minimal entry (path + name, no code).
    /// Binary files are skipped entirely.
    pub fn index_file_paths(&mut self, config: &IndexConfig) -> Result<()> {
        let mut writer: IndexWriter<TantivyDocument> = self.text_index.writer(50_000_000)?;
        let mut count = 0u64;

        let walker = ignore::WalkBuilder::new(&config.project_root)
            .git_ignore(true)
            .parents(true)
            .standard_filters(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            let rel_path = pathdiff::diff_paths(path, &config.project_root)
                .unwrap_or_else(|| path.to_path_buf());
            let rel_str = rel_path.display().to_string();

            // Skip excluded patterns
            if config.exclude_patterns.iter().any(|p| {
                let pattern = p.trim_end_matches("/*");
                rel_str.starts_with(pattern) || rel_str.contains("/target/")
                    || rel_str.contains("node_modules") || rel_str.contains(".git")
            }) {
                continue;
            }

            // Skip noise/backup paths
            if is_noise_path(&rel_str) {
                continue;
            }

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            // Skip binary files
            if SKIP_EXTENSIONS.contains(&ext) {
                continue;
            }
            // Skip code files (already indexed via AST chunking)
            if CODE_EXTENSIONS.contains(&ext) {
                continue;
            }

            let name = path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();

            let doc = doc!(
                self.schema.get_field(FIELD_PATH).unwrap() => rel_str,
                self.schema.get_field(FIELD_NAME).unwrap() => name,
                self.schema.get_field(FIELD_SIGNATURE).unwrap() => "",
                self.schema.get_field(FIELD_CODE).unwrap() => "",
                self.schema.get_field(FIELD_DOC_COMMENT).unwrap() => "",
                self.schema.get_field(FIELD_LANGUAGE).unwrap() => "",
                self.schema.get_field(FIELD_KIND).unwrap() => "Block",
                self.schema.get_field(FIELD_START_LINE).unwrap() => 0u64,
                self.schema.get_field(FIELD_END_LINE).unwrap() => 0u64,
            );
            writer.add_document(doc)?;
            count += 1;
        }

        writer.commit()?;
        self.text_reader.reload()?;
        tracing::info!("Indexed {} non-code file paths", count);
        Ok(())
    }

    /// Search file names only (live walk, substring match)
    /// Fast enough for any project size, always up-to-date.
    pub fn search_filename_live(query: &str, config: &IndexConfig, top_k: usize) -> Result<Vec<SearchResult>> {
        let start = std::time::Instant::now();
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        let walker = ignore::WalkBuilder::new(&config.project_root)
            .git_ignore(true)
            .parents(true)
            .standard_filters(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            let rel_path = pathdiff::diff_paths(path, &config.project_root)
                .unwrap_or_else(|| path.to_path_buf());
            let rel_str = rel_path.display().to_string();

            // Skip excluded patterns
            if config.exclude_patterns.iter().any(|p| {
                let pattern = p.trim_end_matches("/*");
                rel_str.starts_with(pattern) || rel_str.contains("/target/")
                    || rel_str.contains("node_modules") || rel_str.contains(".git")
            }) {
                continue;
            }

            // Skip noise/backup paths
            if is_noise_path(&rel_str) {
                continue;
            }

            // Substring match on relative path
            if rel_str.to_lowercase().contains(&query_lower) {
                let name = path.file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                results.push(SearchResult {
                    score: 1.0,
                    block: CodeBlock {
                        path: rel_path,
                        abs_path: path.to_path_buf(),
                        kind: crate::types::BlockKind::Block,
                        name,
                        signature: String::new(),
                        doc_comment: String::new(),
                        code: String::new(),
                        language: String::new(),
                        start_line: 1,
                        end_line: 1,
                        imports: Vec::new(),
                    },
                    highlight_lines: Vec::new(),
                    explanation: String::new(),
                });
                if results.len() >= top_k {
                    break;
                }
            }
        }

        tracing::info!("search_filename_live: {} results in {:?}", results.len(), start.elapsed());
        Ok(results)
    }

    /// Search all files content (code + plain text)
    /// Uses live grep via `ignore` crate for non-indexed files.
    pub fn search_all_files(&self, query: &str, config: &IndexConfig, top_k: usize) -> Result<Vec<SearchResult>> {
        let start = std::time::Instant::now();
        let mut results = Vec::new();
        let query_lower = query.to_lowercase();

        let walker = ignore::WalkBuilder::new(&config.project_root)
            .git_ignore(true)
            .parents(true)
            .standard_filters(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if SKIP_EXTENSIONS.contains(&ext) {
                continue;
            }

            let rel_path = pathdiff::diff_paths(path, &config.project_root)
                .unwrap_or_else(|| path.to_path_buf());
            let name = path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();

            // Check filename first (quick match)
            let rel_str = rel_path.display().to_string();

            // Skip noise/backup paths
            if is_noise_path(&rel_str) {
                continue;
            }

            if rel_str.to_lowercase().contains(&query_lower) {
                results.push(SearchResult {
                    score: 1.0,
                    block: CodeBlock {
                        path: rel_path.clone(),
                        abs_path: path.to_path_buf(),
                        kind: crate::types::BlockKind::Block,
                        name: name.clone(),
                        signature: String::new(),
                        doc_comment: String::new(),
                        code: String::new(),
                        language: String::new(),
                        start_line: 1,
                        end_line: 1,
                        imports: Vec::new(),
                    },
                    highlight_lines: Vec::new(),
                    explanation: String::new(),
                });
                if results.len() >= top_k {
                    break;
                }
                continue;
            }

            // Then check file content (for non-binary, non-code files)
            if CODE_EXTENSIONS.contains(&ext) {
                continue; // code files are already indexed, skip live grep
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => {
                    // UTF-8 failed → try CJK encodings (GBK, GB18030, Big5) for Windows
                    match std::fs::read(path) {
                        Ok(bytes) => {
                            let (decoded, _, _) = encoding_rs::GBK.decode(&bytes);
                            if decoded.len() > 0 {
                                decoded.to_string()
                            } else {
                                let (decoded, _, _) = encoding_rs::GB18030.decode(&bytes);
                                if decoded.len() > 0 {
                                    decoded.to_string()
                                } else {
                                    let (decoded, _, _) = encoding_rs::BIG5.decode(&bytes);
                                    if decoded.len() > 0 {
                                        decoded.to_string()
                                    } else {
                                        continue;
                                    }
                                }
                            }
                        }
                        Err(_) => continue,
                    }
                }
            };

            if let Some(line_idx) = content.lines().position(|l: &str| l.to_lowercase().contains(&query_lower)) {
                results.push(SearchResult {
                    score: 0.9,
                    block: CodeBlock {
                        path: rel_path,
                        abs_path: path.to_path_buf(),
                        kind: crate::types::BlockKind::Block,
                        name,
                        signature: String::new(),
                        doc_comment: String::new(),
                        code: content.lines().nth(line_idx).unwrap_or("").to_string(),
                        language: String::new(),
                        start_line: line_idx + 1,
                        end_line: line_idx + 1,
                        imports: Vec::new(),
                    },
                    highlight_lines: vec![line_idx + 1],
                    explanation: String::new(),
                });
                if results.len() >= top_k {
                    break;
                }
            }
        }

        tracing::info!("search_all_files: {} results in {:?}", results.len(), start.elapsed());
        Ok(results)
    }

    /// Live grep over CODE files only (gitignore-aware, binary-skipping).
    ///
    /// Returns grep-sized line hits capped at `top_k` as `LocateMatch`, skipping
    /// any relative path already present in `skip_paths`. Used by locate mode as a
    /// **budget-capped** fallback when BM25 under-delivers (e.g. the best hit is
    /// in a code file the chunker missed, or an unindexed path).
    ///
    /// Unlike `search_all_files`, this searches CODE files (the old locate
    /// fallback skipped them, so code queries fell through to `.md` docs and
    /// blew the token budget).
    pub fn search_code_live(
        &self,
        terms: &[String],
        top_k: usize,
        skip_paths: &HashSet<String>,
    ) -> Result<Vec<LocateMatch>> {
        let mut out: Vec<LocateMatch> = Vec::new();
        if terms.is_empty() || top_k == 0 {
            return Ok(out);
        }

        let walker = ignore::WalkBuilder::new(&self.config.project_root)
            .git_ignore(true)
            .parents(true)
            .standard_filters(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let is_file = entry.file_type().map(|f| f.is_file()).unwrap_or(false);
            if !is_file {
                continue;
            }

            let path = entry.path();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if !CODE_EXTENSIONS.contains(&ext.as_str()) {
                continue;
            }

            let rel_path = pathdiff::diff_paths(path, &self.config.project_root)
                .unwrap_or_else(|| path.to_path_buf());
            let rel_str = rel_path.display().to_string();
            if skip_paths.contains(&rel_str) {
                continue;
            }
            // Skip excluded patterns
            if self.config.exclude_patterns.iter().any(|p| {
                let pattern = p.trim_end_matches("/*");
                rel_str.starts_with(pattern)
                    || rel_str.contains("/target/")
                    || rel_str.contains("node_modules")
                    || rel_str.contains(".git")
            }) {
                continue;
            }
            // Skip noise/backup paths
            if is_noise_path(&rel_str) {
                continue;
            }

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // First line containing any term → one grep-sized hit for this file.
            if let Some((idx, line)) = content.lines().enumerate().find(|(_, l)| {
                let low = l.to_lowercase();
                terms.iter().any(|t| low.contains(t))
            }) {
                let trimmed = line.trim();
                let ctx: String = if trimmed.chars().count() > 48 {
                    format!("{}…", trimmed.chars().take(48).collect::<String>())
                } else {
                    trimmed.to_string()
                };
                out.push(LocateMatch {
                    score: 0.85,
                    file: rel_str,
                    abs_path: path.display().to_string(),
                    line: idx + 1,
                    context: ctx,
                    kind: ext,
                    name: String::new(),
                });
                if out.len() >= top_k {
                    break;
                }
            }
        }

        Ok(out)
    }

    /// Search via BM25 full-text
    pub fn search_text(&self, query: &str, top_k: usize) -> Result<Vec<(f32, &IndexedBlock)>> {
        let searcher = self.text_reader.searcher();
        let path_field = self.schema.get_field(FIELD_PATH)?;
        let name_field = self.schema.get_field(FIELD_NAME)?;
        let sig_field = self.schema.get_field(FIELD_SIGNATURE)?;
        let code_field = self.schema.get_field(FIELD_CODE)?;
        let doc_field = self.schema.get_field(FIELD_DOC_COMMENT)?;

        // --- Robust query tokenization (fix for `Foo::bar`, `a.b.c`, `Vec<X>`)
        // Tantivy's QueryParser treats `::` / `(` / `*` etc. as illegal syntax
        // and REJECTS the whole query (Syntax Error). Code identifiers are full
        // of these characters, so we instead tokenize the query ourselves with
        // the SAME rule used at index time (alphanumeric + `_`, everything
        // Tantivy's SimpleTokenizer splits on `!is_alphanumeric()`, which
        // includes `_` as a separator. So `select_best_cfg` → {select, best, cfg},
        // `Cli::parse` → {cli, parse}, `files.len` → {files, len}.
        // We match exactly this split so TermQueries align with index terms.
        let mut terms: Vec<String> = query
            .split(|c: char| !c.is_alphanumeric())
            .map(|s| s.to_lowercase())
            .filter(|s| !s.is_empty())
            .filter(|s| s.len() <= 40)
            .collect();
        let mut seen = std::collections::HashSet::new();
        terms.retain(|t| seen.insert(t.clone()));
        if terms.is_empty() {
            return Ok(Vec::new());
        }

        // Per-field OR of all terms; fields themselves OR'd. BM25 score ranks
        // blocks containing more/all terms higher, so precise symbols surface first.
        let fields = [code_field, name_field, sig_field, doc_field, path_field];
        let tantivy_query: Box<dyn Query> = if terms.len() >= 3 {
            // minimum_should_match ≈ N-1: restructure so first (N-1) terms MUST appear
            // in at least one field (per-term cross-field OR), and the last term is optional.
            // Tantivy 0.22 lacks native min_should_match, so Must/Should composition
            // is the only clean way to enforce multi-term precision.
            let min_match = terms.len() - 1;
            let per_term: Vec<Box<dyn Query>> = terms.iter().map(|term| {
                let mut cls: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for &field in &fields {
                    let tq = TermQuery::new(
                        Term::from_field_text(field, term),
                        IndexRecordOption::WithFreqsAndPositions,
                    );
                    cls.push((Occur::Should, Box::new(tq)));
                }
                Box::new(BooleanQuery::new(cls)) as Box<dyn Query>
            }).collect();

            let mut outer_clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for (i, q) in per_term.into_iter().enumerate() {
                if i < min_match {
                    outer_clauses.push((Occur::Must, q));
                } else {
                    outer_clauses.push((Occur::Should, q));
                }
            }
            Box::new(BooleanQuery::new(outer_clauses))
        } else {
            // 1-2 term queries: original per-field OR structure (correct and efficient)
            let mut field_clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for field in fields {
                let mut term_clauses: Vec<(Occur, Box<dyn Query>)> = Vec::new();
                for term in &terms {
                    let tq = TermQuery::new(
                        Term::from_field_text(field, term),
                        IndexRecordOption::WithFreqsAndPositions,
                    );
                    term_clauses.push((Occur::Should, Box::new(tq)));
                }
                if !term_clauses.is_empty() {
                    let fq: Box<dyn Query> = Box::new(BooleanQuery::new(term_clauses));
                    field_clauses.push((Occur::Should, fq));
                }
            }
            Box::new(BooleanQuery::new(field_clauses))
        };

        let top_docs = searcher
            .search(&tantivy_query, &tantivy::collector::TopDocs::with_limit(top_k * 2))?;

        let mut results = Vec::new();
        for (score, doc_addr) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc::<TantivyDocument>(doc_addr)?;
            let path_str = retrieved_doc
                .get_first(path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Find matching vector store entry by path
            if let Some(entry) = self.vector_store.iter().find(|e| {
                e.block.path.display().to_string() == path_str
            }) {
                // Normalize BM25 score to 0-1 range
                let mut norm_score = (score / 10.0).clamp(0.0, 1.0);

                // Definition priority boost: +0.3 when the block name contains a query
                // term AND the block kind is a definition (function/class/struct/etc.)
                let name_lower = entry.block.name.to_lowercase();
                let is_definition = matches!(
                    entry.block.kind,
                    crate::types::BlockKind::Function
                        | crate::types::BlockKind::Class
                        | crate::types::BlockKind::Struct
                        | crate::types::BlockKind::Method
                        | crate::types::BlockKind::Enum
                        | crate::types::BlockKind::Interface
                        | crate::types::BlockKind::Trait
                );
                if is_definition && terms.iter().any(|t| name_lower.contains(t)) {
                    norm_score += 0.3;
                }

                results.push((norm_score, entry));
            }
        }

        results.truncate(top_k);
        Ok(results)
    }

    /// Search via vector similarity
    pub fn search_vector(&self, query_embedding: &[f32], top_k: usize) -> Result<Vec<(f32, &IndexedBlock)>> {
        let mut scored: Vec<(f32, &IndexedBlock)> = self
            .vector_store
            .iter()
            .filter(|entry| !entry.embedding.is_empty())
            .map(|entry| {
                let sim = crate::embed::cosine_similarity(query_embedding, &entry.embedding);
                (sim, entry)
            })
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        Ok(scored)
    }

    /// Hybrid search: combine BM25 and vector scores with RRF fusion
    pub fn search_hybrid(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        top_k: usize,
    ) -> Result<Vec<(f32, &IndexedBlock)>> {
        let text_results = self.search_text(query, top_k * 2)?;
        let mut candidates: HashMap<usize, f32> = HashMap::new();

        // BM25 RRF contribution
        for (rank, (_score, entry)) in text_results.iter().enumerate() {
            let idx = self.vector_store.iter().position(|e| {
                std::ptr::eq(e, *entry)
            }).unwrap_or(usize::MAX);
            if idx != usize::MAX {
                *candidates.entry(idx).or_insert(0.0) += 1.0 / (60.0 + rank as f32);
            }
        }

        // Vector RRF contribution
        if let Some(emb) = query_embedding {
            let vector_results = self.search_vector(emb, top_k * 2)?;
            for (rank, (_score, entry)) in vector_results.iter().enumerate() {
                let idx = self.vector_store.iter().position(|e| {
                    std::ptr::eq(e, *entry)
                }).unwrap_or(usize::MAX);
                if idx != usize::MAX {
                    *candidates.entry(idx).or_insert(0.0) += 1.0 / (60.0 + rank as f32);
                }
            }
        }

        // Sort by RRF score
        let mut sorted: Vec<(f32, &IndexedBlock)> = candidates
            .into_iter()
            .map(|(idx, score)| (score, &self.vector_store[idx]))
            .collect();
        sorted.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        sorted.truncate(top_k);

        Ok(sorted)
    }

    /// Get all indexed blocks
    pub fn all_blocks(&self) -> &[IndexedBlock] {
        &self.vector_store
    }

    /// Get config reference
    pub fn config(&self) -> &IndexConfig {
        &self.config
    }

    /// Number of indexed blocks
    pub fn len(&self) -> usize {
        self.vector_store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vector_store.is_empty()
    }

    /// Load stored documents from tantivy into vector_store
    fn load_vector_store_from_tantivy(&mut self) -> Result<()> {
        let searcher = self.text_reader.searcher();
        let path_field = self.schema.get_field(FIELD_PATH)?;
        let name_field = self.schema.get_field(FIELD_NAME)?;
        let sig_field = self.schema.get_field(FIELD_SIGNATURE)?;
        let code_field = self.schema.get_field(FIELD_CODE)?;
        let doc_field = self.schema.get_field(FIELD_DOC_COMMENT)?;
        let lang_field = self.schema.get_field(FIELD_LANGUAGE)?;
        let kind_field = self.schema.get_field(FIELD_KIND)?;
        let sl_field = self.schema.get_field(FIELD_START_LINE)?;
        let el_field = self.schema.get_field(FIELD_END_LINE)?;

        // Scan all documents in the index
        let num_docs = searcher.num_docs() as usize;
        let top_docs = searcher.search(
            &tantivy::query::AllQuery,
            &tantivy::collector::TopDocs::with_limit(num_docs.max(1)),
        )?;

        for (_score, doc_addr) in top_docs {
            let retrieved_doc: TantivyDocument = searcher.doc::<TantivyDocument>(doc_addr)?;
            let path_str = retrieved_doc
                .get_first(path_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name_str = retrieved_doc
                .get_first(name_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let sig_str = retrieved_doc
                .get_first(sig_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let code_str = retrieved_doc
                .get_first(code_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let doc_str = retrieved_doc
                .get_first(doc_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let lang_str = retrieved_doc
                .get_first(lang_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let kind_str = retrieved_doc
                .get_first(kind_field)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let start_line = retrieved_doc
                .get_first(sl_field)
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;
            let end_line = retrieved_doc
                .get_first(el_field)
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as usize;

            let rel_path = std::path::PathBuf::from(&path_str);
            let abs_path = self.config.project_root.join(&rel_path);
            let block = CodeBlock {
                path: rel_path,
                abs_path,
                kind: serde_json::from_str(&format!("\"{}\"", kind_str.to_lowercase())).unwrap_or(crate::types::BlockKind::Block),
                name: name_str,
                signature: sig_str,
                doc_comment: doc_str,
                code: code_str,
                language: lang_str,
                start_line,
                end_line,
                imports: Vec::new(),
            };

            let vidx = self.vector_store.len();
            self.vector_store.push(IndexedBlock {
                block,
                embedding: Vec::new(),
            });
            self.doc_id_to_vector.insert(vidx as u32, vidx);
        }

        Ok(())
    }

    fn create_tantivy_doc(&self, block: &CodeBlock) -> TantivyDocument {
        let path_field = self.schema.get_field(FIELD_PATH).unwrap();
        let name_field = self.schema.get_field(FIELD_NAME).unwrap();
        let sig_field = self.schema.get_field(FIELD_SIGNATURE).unwrap();
        let code_field = self.schema.get_field(FIELD_CODE).unwrap();
        let doc_field = self.schema.get_field(FIELD_DOC_COMMENT).unwrap();
        let lang_field = self.schema.get_field(FIELD_LANGUAGE).unwrap();
        let kind_field = self.schema.get_field(FIELD_KIND).unwrap();
        let sl_field = self.schema.get_field(FIELD_START_LINE).unwrap();
        let el_field = self.schema.get_field(FIELD_END_LINE).unwrap();

        doc!(
            path_field => block.path.display().to_string(),
            name_field => block.name.clone(),
            sig_field => block.signature.clone(),
            code_field => block.code.clone(),
            doc_field => block.doc_comment.clone(),
            lang_field => block.language.clone(),
            kind_field => format!("{:?}", block.kind),
            sl_field => block.start_line as u64,
            el_field => block.end_line as u64,
        )
    }
}
