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
use crate::types::format::AiSearchOutput;
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

pub async fn serve(default_root: &Path, custom_index: Option<&PathBuf>, host: &str, port: u16) -> anyhow::Result<()> {
    let root = cache::detect_project_root(default_root);
    let state = Arc::new(AppState {
        engines: Mutex::new(HashMap::new()),
        default_root: root,
        default_index_path: custom_index.cloned(),
    });

    let app = Router::new()
        .route("/search", post(handle_search))
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
) -> Json<AiSearchOutput> {
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
            return Json(AiSearchOutput {
                query: query.query.clone(),
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                _ai_instructions: "error: failed to initialize search engine",
            });
        }
    };

    let mut engines = state.engines.lock().await;
    let pe = match engines.get_mut(&key) {
        Some(pe) => pe,
        None => {
            return Json(AiSearchOutput {
                query: query.query.clone(),
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                _ai_instructions: "error: engine not found",
            });
        }
    };

    let mut search_query = query.clone();
    search_query.mode = mode;
    if search_query.top_k == 0 {
        search_query.top_k = 3;
    }
    if search_query.context_lines == 0 {
        search_query.context_lines = 5;
    }

    let context_lines = search_query.context_lines;
    let query_str = search_query.query.clone();

    match pe.engine.search(search_query) {
        Ok(mut resp) => {
            postprocess::post_process_results(&mut resp, &query_str, context_lines);
            Json(resp.into())
        }
        Err(e) => {
            tracing::error!("Search error: {:?}", e);
            Json(AiSearchOutput {
                query: query_str,
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                _ai_instructions: "error: search failed",
            })
        }
    }
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
                "name": "search_code",
                "description": "Search project code using BM25 full-text + optional filename match. Auto-indexes if needed. Supports multi-project via path field.",
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
                            "description": "Lines around each match (default 5, 0=full block)",
                            "default": 5
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
