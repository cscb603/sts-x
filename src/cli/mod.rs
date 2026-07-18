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
 * - Token-optimized defaults: top_k=3, context=5 for AI consumption
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
        /// Number of results (default: 3)
        #[arg(short, long, default_value = "3")]
        top_k: usize,
        /// Context lines around match for --expand (default: 0 = full block; >0 = window)
        #[arg(short = 'c', long, default_value = "0")]
        context: usize,
        /// Human-readable output instead of default JSON
        #[arg(short = 'H', long)]
        human: bool,
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
        Commands::Search { query, path, index_path, filename, all, locate, top_k, context, human, .. } => {
            let p = resolve_path(path);
            let mode = if *locate {
                crate::types::OutputMode::Locate
            } else {
                crate::types::OutputMode::Expand
            };
            cmd_search(query, &p, index_path.as_ref(), *filename, *all, mode, *top_k, *context, *human).await
        }
        Commands::File { query, path, name_only, top_k, no_rg } => {
            let p = match path {
                Some(p) => normalize_path(p),
                None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            };
            cmd_file(&query, &p, *name_only, *top_k, *no_rg).await
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
        output_mode,
        top_k,
        context_lines,
        ..Default::default()
    };

    let mut response = engine.search(query)?;

    if human_output {
        // Human readable is always the full-block (expand) view.
        postprocess::post_process_results(&mut response, query_str, context_lines);
        print!("{}", format_human_readable(&response));
    } else if matches!(output_mode, crate::types::OutputMode::Locate) {
        let ai_output: crate::types::format::AiLocateOutput = response.into();
        println!("{}", serde_json::to_string_pretty(&ai_output)?);
    } else {
        postprocess::post_process_results(&mut response, query_str, context_lines);
        let ai_output: crate::types::format::AiSearchOutput = response.into();
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
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    let matches = crate::filesearch::search_files(query_str, dir, name_only, top_k, !no_rg)?;
    let elapsed = start.elapsed().as_millis() as u64;
    let out = crate::types::format::AiFileOutput::from_matches(
        query_str.to_string(),
        matches,
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
