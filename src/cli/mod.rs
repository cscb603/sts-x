/*
 * cli/mod.rs
 * Project: sts-x
 * Description: Human/AI-optimized CLI interface
 *
 * Key improvements for AI usage:
 * - No project pollution: indexes go to system cache dir by default
 * - Auto-detect project root: walks up to find .git/Cargo.toml/etc.
 * - Auto-index + stale rebuild: search/serve auto-index if missing or stale
 * - Smart context: --context N controls snippet size, highlight_lines pinpoints matches
 * - Zero-config: just run `sts-x search "query"` in any project directory
 * - Token-optimized defaults: top_k=2, context=0 for AI consumption
 */

use crate::types::{IndexConfig, SearchQuery, SearchMode, format::format_human_readable};
use crate::chunker::Chunker;
use crate::indexer::SearchIndex;
use crate::search::SearchEngine;
use crate::cache;
use crate::postprocess;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "sts-x", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Index a project directory (manual; usually not needed — search auto-indexes)
    Index {
        /// Project root path (default: auto-detected from current directory)
        path: Option<PathBuf>,
        /// Custom index output directory (default: system cache dir)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Languages to index (comma-separated, default: all supported)
        #[arg(short, long)]
        languages: Option<String>,
    },
    /// Search a project (auto-indexes if needed, auto-detects project root)
    Search {
        /// Natural language query
        query: String,
        /// Project root path (default: auto-detected from current directory)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Custom index directory (default: system cache dir)
        #[arg(short = 'o', long)]
        index_path: Option<PathBuf>,
        /// Search file names only (fast live walk, no index needed)
        #[arg(short = 'f', long)]
        filename: bool,
        /// Search all files (code + non-code, filename + content)
        #[arg(long)]
        all: bool,
        /// Output mode: --expand (default) returns full AST blocks (read/modify);
        /// --locate returns only matching lines + small context (grep-sized, ~130 tok).
        #[arg(long)]
        locate: bool,
        /// Explicitly request --expand (full blocks). Default when neither flag is given.
        #[arg(long)]
        expand: bool,
        /// Number of results (default: 2)
        #[arg(short, long, default_value = "2")]
        top_k: usize,
        /// Context lines around match for --expand (default: 0 = full block; >0 = window)
        #[arg(short = 'c', long, default_value = "0")]
        context: usize,
        /// Human-readable output instead of default JSON
        #[arg(short = 'H', long)]
        human: bool,
        /// Cap total output to roughly this many tokens (0 = unlimited).
        /// Estimation: (char_count + 1) / 2. Results are truncated by dropping
        /// lowest-score entries until the budget is met.
        #[arg(long, default_value = "0")]
        max_tokens: usize,
        /// Restrict results to files whose path contains this substring
        /// (e.g. --path-filter cache.rs or --path-filter src/cache.rs)
        #[arg(long)]
        path_filter: Option<String>,
        /// Omit the `_ai_instructions` field from expand output (saves ~200 tok)
        #[arg(long)]
        no_hint: bool,
        /// Sort results by file modification time, most recent first
        #[arg(long)]
        sort_recent: bool,
    },
    /// AI one-shot search (CLI path): auto-routes symbol→locate, NL→expand+budget.
    /// Shares src/router.rs with the MCP `search` tool — identical behavior on both paths.
    Ai {
        /// Query: bare symbol (e.g. "run_search") or natural language description
        query: String,
        /// Project root path (default: auto-detected from current directory)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Override the auto token budget (0 = use router default)
        #[arg(long, default_value = "0")]
        max_tokens: usize,
    },
    /// File search: filename + content across ANY directory (no index needed).
    /// Uses ripgrep if available, else a gitignore-aware walk. Zero-config.
    File {
        /// Search query (filename fragment or content term)
        query: String,
        /// Directory to search (default: current directory)
        #[arg(short = 'p', long)]
        path: Option<PathBuf>,
        /// Match content (default) in addition to filename. Use --name-only to skip.
        #[arg(long)]
        name_only: bool,
        /// Maximum results (default: 20)
        #[arg(short, long, default_value = "20")]
        top_k: usize,
        /// Force using the built-in walker instead of ripgrep
        #[arg(long)]
        no_rg: bool,
        /// Cap total output to roughly this many tokens (0 = unlimited).
        /// Estimation: (char_count + 1) / 2. Results are truncated by dropping
        /// lowest-score entries until the budget is met.
        #[arg(long, default_value = "0")]
        max_tokens: usize,
    },
    /// Start MCP HTTP server (auto-indexes, supports multi-project via "path" field)
    Serve {
        /// Project root path (default: auto-detected from current directory)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Custom index directory (default: system cache dir)
        #[arg(short = 'o', long)]
        index_path: Option<PathBuf>,
        /// Host address (default: 127.0.0.1)
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port (default: 9876)
        #[arg(short = 'P', long, default_value = "9876")]
        port: u16,
    },
    /// Show index status and cache location
    Status {
        /// Project root path (default: auto-detected from current directory)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Custom index directory (default: system cache dir)
        #[arg(short = 'o', long)]
        index_path: Option<PathBuf>,
    },
}

pub async fn run(cli: &Cli) -> anyhow::Result<()> {
    match &cli.command {
        Commands::Index { path, output, languages } => {
            let p = resolve_path(path);
            cmd_index(&p, output, languages).await
        }
        Commands::Search { query, path, index_path, filename, all, locate, top_k, context, human, max_tokens, path_filter, no_hint, sort_recent, .. } => {
            let p = resolve_path(path);
            let mode = if *locate {
                crate::types::OutputMode::Locate
            } else {
                crate::types::OutputMode::Expand
            };
            cmd_search(query, &p, index_path.as_ref(), *filename, *all, mode, *top_k, *context, *human, *max_tokens, path_filter.as_deref(), *no_hint, *sort_recent).await
        }
        Commands::Ai { query, path, max_tokens } => {
            let p = resolve_path(path);
            run_ai(query, &p, *max_tokens).await
        }
        Commands::File { query, path, name_only, top_k, no_rg, max_tokens } => {
            let p = match path {
                Some(p) => normalize_path(p),
                None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            };
            cmd_file(query, &p, *name_only, *top_k, *no_rg, *max_tokens).await
        }
        Commands::Serve { path, index_path, host, port } => {
            let p = resolve_path(path);
            cmd_serve(&p, index_path.as_ref(), host, *port).await
        }
        Commands::Status { path, index_path } => {
            let p = resolve_path(path);
            cmd_status(&p, index_path.as_ref()).await
        }
    }
}

/// R1: AI one-shot entry (CLI path). Classifies the query via the shared
/// `router` module (same logic as the MCP `search` tool) and reuses the
/// existing `cmd_search` pipeline — no duplicated search logic.
async fn run_ai(query: &str, root: &Path, max_tokens_override: usize) -> anyhow::Result<()> {
    let decision = crate::router::classify(query);
    let max_tokens = if max_tokens_override > 0 {
        max_tokens_override
    } else {
        decision.max_tokens
    };
    cmd_search(
        query,
        root,
        None,               // index_path: default cache dir
        false,              // filename mode
        false,              // all mode
        decision.output_mode,
        decision.top_k,
        0,                  // context_lines: full block for expand
        false,              // human output: JSON for AI
        max_tokens,
        None,               // path_filter
        false,              // no_hint
        false,              // sort_recent
    )
    .await
}

/// Normalize POSIX-style paths for Windows (e.g. /c/Users → C:\Users)
/// On non-Windows, this is a no-op.
fn normalize_path(p: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let s = p.to_string_lossy();
        // Convert /c/... or /C/... to C:\...
        if s.starts_with('/') && s.len() >= 3 && s.as_bytes()[2] == b'/' {
            let drive = s.as_bytes()[1].to_ascii_uppercase() as char;
            if drive.is_ascii_alphabetic() {
                let rest = &s[3..].replace('/', "\\");
                return PathBuf::from(format!("{}:\\{}", drive, rest));
            }
        }
        // Convert C:/... to C:\...
        if s.len() >= 3 && s.as_bytes()[1] == b':' && s.as_bytes()[2] == b'/' {
            return PathBuf::from(s.replace('/', "\\"));
        }
    }
    p.to_path_buf()
}

fn resolve_path(explicit: &Option<PathBuf>) -> PathBuf {
    let start = match explicit {
        Some(p) => normalize_path(p),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    cache::detect_project_root(&start)
}

fn build_config(project_root: &Path, custom_index: Option<&PathBuf>) -> IndexConfig {
    IndexConfig {
        project_root: project_root.to_path_buf(),
        index_path: cache::resolve_index_path(project_root, custom_index),
        ..IndexConfig::default()
    }
}

async fn ensure_indexed(config: &IndexConfig) -> anyhow::Result<bool> {
    let tantivy_dir = config.index_path.join("tantivy");
    let meta = tantivy_dir.join("meta.json");

    if meta.exists() && !cache::is_index_stale(&config.index_path, &config.project_root) {
        tracing::debug!("Index exists and fresh at {}", config.index_path.display());
        return Ok(false);
    }

    if meta.exists() {
        tracing::info!("Index is stale, rebuilding for: {}", config.project_root.display());
        eprintln!("[sts-x] Index stale, rebuilding {} ...", config.project_root.display());
        std::fs::remove_dir_all(&config.index_path).ok();
    } else {
        tracing::info!("No index found, auto-indexing project: {}", config.project_root.display());
        eprintln!("[sts-x] Building index for {} ...", config.project_root.display());
    }

    std::fs::create_dir_all(&config.index_path)?;

    let mut chunker = Chunker::new(&config.languages)?;
    let blocks = chunker.index_project(&config.project_root, config)?;
    tracing::info!("Found {} code blocks", blocks.len());

    let mut index = SearchIndex::new(config.clone(), None)?;
    index.index_blocks(blocks)?;
    index.index_file_paths(config)?;

    eprintln!("[sts-x] Index ready ({} blocks) at {}", index.len(), config.index_path.display());
    Ok(true)
}

async fn cmd_index(
    project_root: &Path,
    output: &Option<PathBuf>,
    languages: &Option<String>,
) -> anyhow::Result<()> {
    let mut config = build_config(project_root, output.as_ref());
    if let Some(langs) = languages {
        config.languages = langs.split(',').map(|s| s.trim().to_string()).collect();
    }

    tracing::info!("Indexing project: {}", project_root.display());
    eprintln!("[sts-x] Indexing {} ...", project_root.display());

    std::fs::create_dir_all(&config.index_path)?;

    let mut chunker = Chunker::new(&config.languages)?;
    let blocks = chunker.index_project(project_root, &config)?;
    eprintln!("[sts-x] Parsed {} code blocks", blocks.len());

    let mut index = SearchIndex::new(config.clone(), None)?;
    index.index_blocks(blocks)?;
    index.index_file_paths(&config)?;

    eprintln!("[sts-x] Indexed {} blocks → {}", index.len(), config.index_path.display());
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn cmd_search(
    query_str: &str,
    root: &Path,
    custom_index: Option<&PathBuf>,
    filename_mode: bool,
    all_mode: bool,
    output_mode: crate::types::OutputMode,
    top_k: usize,
    context_lines: usize,
    human_output: bool,
    max_tokens: usize,
    path_filter: Option<&str>,
    no_hint: bool,
    sort_recent: bool,
) -> anyhow::Result<()> {
    let config = build_config(root, custom_index);

    if !filename_mode {
        ensure_indexed(&config).await?;
    }

    let mode = if all_mode {
        SearchMode::All
    } else if filename_mode {
        SearchMode::Filename
    } else {
        SearchMode::Code
    };

    let index = SearchIndex::new(config.clone(), None)?;
    let mut engine = SearchEngine::new(Arc::new(index), None);

    let query = SearchQuery {
        query: query_str.to_string(),
        mode,
        output_mode: Some(output_mode),
        top_k,
        context_lines,
        max_tokens,
        path_filter: path_filter.map(|s| s.to_string()),
        hint: !no_hint,
        ..Default::default()
    };

    let mut response = engine.search(query)?;

    // P1-1: `--sort recent` — reorder results by file mtime (newest first).
    if sort_recent {
        response.results.sort_by(|a, b| {
            let mtime = |r: &crate::types::SearchResult| -> u64 {
                std::fs::metadata(&r.block.abs_path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
            };
            mtime(b).cmp(&mtime(a))
        });
    }

    if human_output {
        // Human readable is always the full-block (expand) view.
        postprocess::post_process_results(&mut response, query_str, context_lines);
        print!("{}", format_human_readable(&response));
    } else if matches!(output_mode, crate::types::OutputMode::Locate) {
        let ai_output: crate::types::format::AiLocateOutput = response.into();
        println!("{}", serde_json::to_string_pretty(&ai_output)?);
    } else {
        postprocess::post_process_results(&mut response, query_str, context_lines);
        let mut ai_output: crate::types::format::AiSearchOutput = response.into();
        // P1-2: `--no-hint` drops _ai_instructions (~200 tok saved per call).
        if no_hint {
            ai_output._ai_instructions = None;
        }
        // R3: fold hot symbols (same shape on CLI and MCP paths).
        postprocess::aggregate_results(&mut ai_output);
        println!("{}", serde_json::to_string_pretty(&ai_output)?);
    }

    Ok(())
}

/// `file` subcommand: filename + content search across ANY directory,
/// with zero index required. Prefers ripgrep; falls back to a gitignore-aware
/// walker. Mirrors the `sts` file-search UX for AI consumption.
async fn cmd_file(
    query_str: &str,
    dir: &Path,
    name_only: bool,
    top_k: usize,
    no_rg: bool,
    max_tokens: usize,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let matches = crate::filesearch::search_files(query_str, dir, name_only, top_k, !no_rg)?;
    let elapsed = start.elapsed().as_millis() as u64;

    // Apply max_tokens truncation: estimate token count from context fields
    let out_matches: Vec<crate::types::FileMatch> = if max_tokens > 0 {
        let mut total: usize = 0;
        let mut truncated = Vec::new();
        for m in matches {
            let tok = m.context.chars().count().div_ceil(2);
            if total + tok > max_tokens && !truncated.is_empty() {
                break;
            }
            total += tok;
            truncated.push(m);
        }
        truncated
    } else {
        matches
    };

    let out = crate::types::format::AiFileOutput::from_matches(
        query_str.to_string(),
        out_matches,
        elapsed,
    );
    // total_hits is the actual match count
    let mut out = out;
    out.total_hits = out.matches.len();
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

async fn cmd_serve(root: &Path, custom_index: Option<&PathBuf>, host: &str, port: u16) -> anyhow::Result<()> {
    tracing::info!("Starting STS-X MCP server for project: {}", root.display());
    eprintln!("[sts-x] Serving {} on {}:{}", root.display(), host, port);
    eprintln!("[sts-x] POST {{\"query\":\"...\"}} to http://{}:{}/search", host, port);
    eprintln!("[sts-x] Index stored at system cache (no project pollution)");

    crate::server::serve(root, custom_index, host, port).await?;
    Ok(())
}

async fn cmd_status(root: &Path, custom_index: Option<&PathBuf>) -> anyhow::Result<()> {
    let config = build_config(root, custom_index);
    let index_path = &config.index_path;

    println!("Project root: {}", config.project_root.display());
    println!("Index path:   {}", index_path.display());
    println!("Cache root:   {}", cache::cache_root().display());

    if !index_path.exists() {
        println!("Status:       NOT INDEXED");
        println!("Run `sts-x search \"query\"` in the project directory to auto-index.");
        return Ok(());
    }

    let tantivy_path = index_path.join("tantivy");
    if tantivy_path.join("meta.json").exists() {
        if cache::is_index_stale(index_path, root) {
            println!("Status:       STALE (files changed since last index)");
            println!("Next search will auto-rebuild.");
        } else {
            println!("Status:       READY");
        }
        match SearchIndex::new(config, None) {
            Ok(idx) => {
                println!("Blocks:       {}", idx.len());
            }
            Err(e) => {
                println!("Index error:  {}", e);
            }
        }
    } else {
        println!("Status:       INCOMPLETE");
    }

    Ok(())
}
