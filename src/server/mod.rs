/*
 * server/mod.rs
 * Project: sts-x
 * Description: MCP-compatible HTTP server for AI consumption
 *
 * Key features for AI:
 * - Multi-project: accepts optional "path" in query body to switch projects
 * - Auto-index: transparently builds/rebuilds index as needed
 * - Post-processed results: highlight_lines + context window
 * - Pure JSON to response body; logs to stderr
 */

use crate::chunker::Chunker;
use crate::indexer::SearchIndex;
use crate::postprocess;
use crate::search::SearchEngine;
use crate::types::{IndexConfig, SearchMode, SearchQuery};
use crate::types::format::{AiFileOutput, AiLocateOutput, AiSearchOutput};
use crate::cache;
use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::Mutex;

struct ProjectEngine {
    engine: SearchEngine,
    config: IndexConfig,
}

pub struct AppState {
    engines: Mutex<HashMap<String, ProjectEngine>>,
    default_root: PathBuf,
    default_index_path: Option<PathBuf>,
}

/// Unified search response (expand or locate JSON shape).
#[derive(serde::Serialize)]
#[serde(untagged)]
enum AiResponse {
    Expand(AiSearchOutput),
    Locate(AiLocateOutput),
}

/// Body for the MCP `/file` endpoint.
#[derive(serde::Deserialize)]
struct FileQuery {
    query: String,
    #[serde(default)]
    path: Option<PathBuf>,
    #[serde(default = "default_content")]
    content: bool,
    #[serde(default = "default_topk_file")]
    top_k: usize,
    #[serde(default)]
    name_only: bool,
}

fn default_content() -> bool {
    true
}
fn default_topk_file() -> usize {
    20
}

pub async fn serve(default_root: &Path, custom_index: Option<&PathBuf>, host: &str, port: u16) -> anyhow::Result<()> {
    let root = cache::detect_project_root(default_root);
    let state = Arc::new(AppState {
        engines: Mutex::new(HashMap::new()),
        default_root: root,
        default_index_path: custom_index.cloned(),
    });

    let app = Router::new()
        .route("/search", post(handle_search))
        .route("/file", post(handle_file))
        .route("/health", get(handle_health))
        .route("/tools", get(handle_tools))
        .route("/", get(handle_root))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    tracing::info!("STS-X server listening on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn get_or_create_engine(
    state: &AppState,
    project_path: Option<&PathBuf>,
) -> anyhow::Result<(String, IndexConfig)> {
    let root = match project_path {
        Some(p) => cache::detect_project_root(p),
        None => state.default_root.clone(),
    };
    let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
    let key = canonical.display().to_string();

    let mut engines = state.engines.lock().await;

    if let Some(pe) = engines.get(&key) {
        if cache::is_index_stale(&pe.config.index_path, &pe.config.project_root) {
            tracing::info!("Index stale for {}, re-indexing...", key);
            engines.remove(&key);
        } else {
            return Ok((key, pe.config.clone()));
        }
    }

    let index_path = state.default_index_path.clone()
        .unwrap_or_else(|| cache::index_dir_for(&canonical));
    let config = IndexConfig {
        project_root: canonical.clone(),
        index_path: index_path.clone(),
        ..IndexConfig::default()
    };

    let tantivy_dir = index_path.join("tantivy");
    let needs_build = !tantivy_dir.join("meta.json").exists()
        || cache::is_index_stale(&index_path, &canonical);

    if needs_build {
        tracing::info!("Building index for {} ...", canonical.display());
        eprintln!("[sts-x] Building index for {} ...", canonical.display());
        std::fs::create_dir_all(&index_path)?;
        let mut chunker = Chunker::new(&config.languages)?;
        let blocks = chunker.index_project(&canonical, &config)?;
        let mut index = SearchIndex::new(config.clone(), None)?;
        index.index_blocks(blocks)?;
        index.index_file_paths(&config)?;
        eprintln!("[sts-x] Index ready ({} blocks)", index.len());
        let engine = SearchEngine::new(Arc::new(index), None);
        engines.insert(key.clone(), ProjectEngine { engine, config: config.clone() });
    } else {
        let index = SearchIndex::new(config.clone(), None)?;
        let engine = SearchEngine::new(Arc::new(index), None);
        engines.insert(key.clone(), ProjectEngine { engine, config: config.clone() });
    }

    Ok((key, config))
}

async fn handle_search(
    State(state): State<Arc<AppState>>,
    Json(query): Json<SearchQuery>,
) -> Json<AiResponse> {
    let project_path = query.path.clone();
    let mode = if query.filename {
        SearchMode::Filename
    } else if query.all {
        SearchMode::All
    } else {
        query.mode.clone()
    };

    let (key, _config) = match get_or_create_engine(&state, project_path.as_ref()).await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!("Failed to get engine: {:?}", e);
            return Json(AiResponse::Expand(AiSearchOutput {
                query: query.query.clone(),
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                _ai_instructions: "error: failed to initialize search engine",
            }));
        }
    };

    let mut engines = state.engines.lock().await;
    let pe = match engines.get_mut(&key) {
        Some(pe) => pe,
        None => {
            return Json(AiResponse::Expand(AiSearchOutput {
                query: query.query.clone(),
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                _ai_instructions: "error: engine not found",
            }));
        }
    };

    let mut search_query = query.clone();
    search_query.mode = mode;
    if search_query.top_k == 0 {
        search_query.top_k = 3;
    }
    // 3.0: expand default = full block (context_lines 0). Do NOT force 5.

    let context_lines = search_query.context_lines;
    let query_str = search_query.query.clone();
    let is_locate = matches!(search_query.output_mode, crate::types::OutputMode::Locate);

    match pe.engine.search(search_query) {
        Ok(mut resp) => {
            if is_locate {
                Json(AiResponse::Locate(resp.into()))
            } else {
                postprocess::post_process_results(&mut resp, &query_str, context_lines);
                Json(AiResponse::Expand(resp.into()))
            }
        }
        Err(e) => {
            tracing::error!("Search error: {:?}", e);
            Json(AiResponse::Expand(AiSearchOutput {
                query: query_str,
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                _ai_instructions: "error: search failed",
            }))
        }
    }
}

async fn handle_file(
    State(_state): State<Arc<AppState>>,
    Json(body): Json<FileQuery>,
) -> Json<AiFileOutput> {
    let dir = match &body.path {
        Some(p) => p.clone(),
        None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
    };
    let start = std::time::Instant::now();
    let effective_name_only = body.name_only || !body.content;
    let matches = match crate::filesearch::search_files(
        &body.query,
        &dir,
        effective_name_only,
        body.top_k,
        true,
    ) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!("file search error: {:?}", e);
            return Json(AiFileOutput {
                query: body.query,
                mode: "file",
                matches: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                _ai_instructions: "error: file search failed",
            });
        }
    };
    let elapsed = start.elapsed().as_millis() as u64;
    let mut out = AiFileOutput::from_matches(body.query, matches, elapsed);
    out.total_hits = out.matches.len();
    Json(out)
}

async fn handle_health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "service": "sts-x"
    }))
}

async fn handle_tools() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "tools": [
            {
                "name": "search",
                "description": "Unified code search (STS-X 3.0). BM25 over AST blocks, auto-indexes if needed, supports multi-project via path. Use output_mode=locate for grep-sized line hits (cheap), or expand (default) for full code blocks.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query (natural language or code fragment)"
                        },
                        "mode": {
                            "type": "string",
                            "enum": ["code", "filename", "all"],
                            "description": "code=AST-aware code search, filename=file name match, all=everything"
                        },
                        "output_mode": {
                            "type": "string",
                            "enum": ["expand", "locate"],
                            "description": "expand=full AST block (default, for read/modify); locate=line-level grep-sized hits (~130 tok) for first-pass location"
                        },
                        "path": {
                            "type": "string",
                            "description": "Project root (auto-detected if omitted)"
                        },
                        "top_k": {
                            "type": "integer",
                            "description": "Number of results (default 3)",
                            "default": 3
                        },
                        "context_lines": {
                            "type": "integer",
                            "description": "Lines around each match in expand mode (default 0 = full block, >0 = window)",
                            "default": 0
                        },
                        "filename": {
                            "type": "boolean",
                            "description": "Shortcut: search file names only"
                        },
                        "all": {
                            "type": "boolean",
                            "description": "Shortcut: search all files"
                        }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "file",
                "description": "File search across ANY directory (no index needed). Searches filename + content via ripgrep (or built-in walker). Perfect for locating assets/configs/prompts in unindexed dirs like ~/Downloads.",
                "input_schema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Filename fragment or content term"
                        },
                        "path": {
                            "type": "string",
                            "description": "Directory to search (default: server cwd)"
                        },
                        "content": {
                            "type": "boolean",
                            "description": "Also match file content (default true). Set false for name-only.",
                            "default": true
                        },
                        "name_only": {
                            "type": "boolean",
                            "description": "Alias for content=false (name match only)"
                        },
                        "top_k": {
                            "type": "integer",
                            "description": "Maximum results (default 20)",
                            "default": 20
                        }
                    },
                    "required": ["query"]
                }
            }
        ]
    }))
}

async fn handle_root() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "service": "sts-x",
        "version": env!("CARGO_PKG_VERSION"),
        "endpoints": {
            "/health": "GET — service health",
            "/tools": "GET — tool discovery (MCP schema)",
            "/search": "POST — code search (application/json)"
        },
        "quick_start": {
            "example_curl": "curl -X POST http://127.0.0.1:9876/search -H 'Content-Type: application/json' -d '{\"query\":\"search function\",\"top_k\":3,\"context_lines\":5}'"
        }
    }))
}
