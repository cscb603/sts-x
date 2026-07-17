/*
 * cli/mod.rs
 * Project: sts-x
 * Description: Human-optimized CLI interface
 *
 * Provides a clap-based CLI for humans to use sts-x directly:
 * - `sts-x index <path>` — Index a project
 * - `sts-x search <query>` — Search indexed project
 * - `sts-x serve` — Start MCP server
 * - `sts-x status` — Show index status
 */

use crate::types::{IndexConfig, SearchQuery, SearchMode, format::format_human_readable};
use crate::chunker::Chunker;
use crate::embed::EmbeddingModel;
use crate::indexer::SearchIndex;
use crate::search::SearchEngine;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;

/// STS-X: AI 代码搜索引擎 — AST 切块 + BM25 + MCP 服务
///
/// 单二进制零依赖，默认 JSON 输出给 AI 消费。
/// 三种搜索模式：Code（代码语义）、Filename（文件名）、All（全文件）。
/// 提供 MCP HTTP 服务（POST /search），供 AI Agent 直接调用。
#[derive(Parser)]
#[command(name = "sts-x", version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Index a project directory
    Index {
        /// Project root path
        path: PathBuf,
        /// Index output directory (default: <project>/.stsx-index)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Languages to index (comma-separated, default: all supported)
        #[arg(short, long)]
        languages: Option<String>,
        /// Embedding model path (requires `semantic` feature: cargo build --features semantic)
        #[arg(short, long)]
        model: Option<PathBuf>,
    },
    /// Search an indexed project
    Search {
        /// Natural language query
        query: String,
        /// Project root or index path
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Custom index directory (default: <project>/.stsx-index)
        #[arg(short = 'o', long)]
        index_path: Option<PathBuf>,
        /// Search mode: -f = filename only, --all = all files content
        #[arg(short = 'f', long)]
        filename: bool,
        /// Search all files (code + non-code, filename + content)
        #[arg(long)]
        all: bool,
        /// Number of results (default: 5)
        #[arg(short, long, default_value = "5")]
        top_k: usize,
        /// Human-readable output instead of default JSON
        #[arg(short = 'H', long)]
        human: bool,
    },
    /// Start MCP HTTP server
    Serve {
        /// Project root or index path (default: current dir)
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Custom index directory (default: <project>/.stsx-index)
        #[arg(short = 'o', long)]
        index_path: Option<PathBuf>,
        /// Host address (default: 127.0.0.1)
        #[arg(short, long, default_value = "127.0.0.1")]
        host: String,
        /// Port (default: 9876)
        #[arg(short, long, default_value = "9876")]
        port: u16,
    },
    /// Show index status
    Status {
        /// Project root or index path
        #[arg(short, long)]
        path: Option<PathBuf>,
        /// Custom index directory (default: <project>/.stsx-index)
        #[arg(short = 'o', long)]
        index_path: Option<PathBuf>,
    },
}

pub async fn run(cli: &Cli) -> anyhow::Result<()> {
    match &cli.command {
        Commands::Index { path, output, languages, model } => {
            cmd_index(path, output, languages, model).await
        }
        Commands::Search { query, path, index_path, filename, all, top_k, human } => {
            cmd_search(query, path, index_path.as_ref(), *filename, *all, *top_k, *human).await
        }
        Commands::Serve { path, index_path, host, port } => {
            cmd_serve(path.as_ref(), index_path.as_ref(), host, *port).await
        }
        Commands::Status { path, index_path } => {
            cmd_status(path, index_path.as_ref()).await
        }
    }
}

async fn cmd_index(
    project_root: &PathBuf,
    output: &Option<PathBuf>,
    languages: &Option<String>,
    model_path: &Option<PathBuf>,
) -> anyhow::Result<()> {
    tracing::info!("Indexing project: {}", project_root.display());

    let mut config = IndexConfig::default();
    config.project_root = project_root.clone();
    if let Some(out) = output {
        config.index_path = out.clone();
    } else {
        config.index_path = project_root.join(".stsx-index");
    }
    if let Some(langs) = languages {
        config.languages = langs.split(',').map(|s| s.trim().to_string()).collect();
    }
    if let Some(mp) = model_path {
        config.model_path = mp.clone();
    }

    // Ensure index directory exists
    std::fs::create_dir_all(&config.index_path)?;

    // Load embedding model if provided
    let embed_model = if let Some(mp) = model_path {
        let tokenizer_path = mp.join("tokenizer.json");
        let model_file = mp.join("model.onnx");
        if model_file.exists() && tokenizer_path.exists() {
            tracing::info!("Loading embedding model from: {}", mp.display());
            Some(EmbeddingModel::load(&model_file, &tokenizer_path, 384, 512)?)
        } else {
            tracing::warn!("Model path provided but model.onnx or tokenizer.json not found at {}. Skipping embeddings.", mp.display());
            None
        }
    } else {
        tracing::info!("No embedding model provided. Indexing with BM25 only (no semantic search).");
        None
    };

    // Step 1: Chunk code
    tracing::info!("Parsing code with tree-sitter AST...");
    let mut chunker = Chunker::new(&config.languages)?;
    let blocks = chunker.index_project(project_root, &config)?;
    tracing::info!("Found {} code blocks", blocks.len());

    // Step 2: Index
    tracing::info!("Building search index...");
    let mut index = SearchIndex::new(config.clone(), embed_model)?;
    index.index_blocks(blocks)?;
    // Also index non-code file paths (for filename search)
    index.index_file_paths(&config)?;

    tracing::info!("Indexed {} blocks + file paths. Ready.", index.len());

    Ok(())
}

async fn cmd_search(
    query_str: &str,
    path: &Option<PathBuf>,
    custom_index: Option<&PathBuf>,
    filename_mode: bool,
    all_mode: bool,
    top_k: usize,
    human_output: bool,
) -> anyhow::Result<()> {
    let root = path.as_deref().unwrap_or(&std::env::current_dir()?).to_path_buf();
    let mut config = IndexConfig::default();
    config.project_root = root.clone();
    config.index_path = custom_index.cloned().unwrap_or_else(|| root.join(".stsx-index"));

    // Check if index exists
    if !config.index_path.join("tantivy").exists() {
        anyhow::bail!("No index found at {}. Run `sts-x index <path>` first.", config.index_path.display());
    }

    // Determine search mode
    let mode = if all_mode {
        SearchMode::All
    } else if filename_mode {
        SearchMode::Filename
    } else {
        SearchMode::Code
    };

    // Load embedding model if available
    let embed_model = None; // For search-only, we don't need the model (vectors are stored)
    let index = SearchIndex::new(config.clone(), embed_model)?;

    let mut engine = SearchEngine::new(Arc::new(index), None);
    let query = SearchQuery {
        query: query_str.to_string(),
        mode,
        top_k,
        ..Default::default()
    };

    let response = engine.search(query)?;

    if human_output {
        print!("{}", format_human_readable(&response));
    } else {
        // Default: JSON for AI consumption
        let ai_output: crate::types::format::AiSearchOutput = response.into();
        println!("{}", serde_json::to_string_pretty(&ai_output)?);
    }

    Ok(())
}

async fn cmd_serve(path: Option<&PathBuf>, custom_index: Option<&PathBuf>, host: &str, port: u16) -> anyhow::Result<()> {
    let root = path.unwrap_or(&std::env::current_dir()?).to_path_buf();
    let mut config = IndexConfig::default();
    config.project_root = root.clone();
    config.index_path = custom_index.cloned().unwrap_or_else(|| root.join(".stsx-index"));

    tracing::info!("Starting STS-X MCP server for project: {}", root.display());
    tracing::info!("Listening on {}:{}. POST /search with SearchQuery JSON", host, port);

    let index = SearchIndex::new(config.clone(), None)?;
    let engine = SearchEngine::new(Arc::new(index), None);

    crate::server::serve(engine, host, port).await?;
    Ok(())
}

async fn cmd_status(path: &Option<PathBuf>, custom_index: Option<&PathBuf>) -> anyhow::Result<()> {
    let root = path.as_deref().unwrap_or(&std::env::current_dir()?).to_path_buf();
    let index_path = custom_index.cloned().unwrap_or_else(|| root.join(".stsx-index"));

    if !index_path.exists() {
        println!("No sts-x index found at: {}", index_path.display());
        println!("Run `sts-x index {}` to create one.", root.display());
        return Ok(());
    }

    let tantivy_path = index_path.join("tantivy");
    let tantivy_meta = tantivy_path.join("meta.json");

    if tantivy_meta.exists() {
        let meta = std::fs::read_to_string(&tantivy_meta)?;
        println!("Index path: {}", index_path.display());
        println!("Index meta: {}", meta);
    } else {
        println!("Index path: {}", index_path.display());
        println!("Size: {} entries (approx)", std::fs::read_dir(&tantivy_path).map(|d| d.count()).unwrap_or(0));
    }

    Ok(())
}
