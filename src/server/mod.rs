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
// HTTP 框架统一由 core_lib::mcp 提供并重新导出（不再直接依赖 axum，见白皮书 §2）。
use core_lib::mcp::axum::{
    extract::State,
    routing::post,
    Json, Router,
};
use core_lib::mcp::{McpServer, Tool};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
/// R1 (v0.4 §7.5): no `#[serde(untagged)]` — serialization delegates to the
/// inner payload, which now carries an explicit `"mode": "expand"|"locate"`
/// discriminator field, so AI parses CLI and MCP responses identically.
enum AiResponse {
    Expand(AiSearchOutput),
    Locate(AiLocateOutput),
}

impl serde::Serialize for AiResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            AiResponse::Expand(out) => out.serialize(serializer),
            AiResponse::Locate(out) => out.serialize(serializer),
        }
    }
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
    /// P1-3: cap output to ~this many tokens (0 = unlimited), mirroring CLI.
    #[serde(default)]
    max_tokens: usize,
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

    // 业务路由（/search /file handler 保持原样，见白皮书 §2）
    let biz = Router::new()
        .route("/search", post(handle_search))
        .route("/file", post(handle_file))
        .with_state(state);

    // /health、/tools、/ 三个“壳层”交由 core_lib::mcp::McpServer 自动生成
    McpServer::new("sts-x", env!("CARGO_PKG_VERSION"), crate::types::format::AI_HINT)
        .tool(search_tool_schema())
        .tool(file_tool_schema())
        .merge(biz)
        .serve(host, port)
        .await
}

/// `search` 工具的 MCP Schema（原 handle_tools 内容，迁移到 core_lib McpServer 声明）。
fn search_tool_schema() -> Tool {
    Tool::new(
        "search",
        "Unified code search (STS-X 3.0). BM25 over AST blocks, auto-indexes if needed, supports multi-project via path. Omit output_mode to AUTO-ROUTE: symbol-like query→locate (grep-sized, cheap), natural language→expand (full blocks, token-budgeted). Or set output_mode explicitly. Response carries a \"mode\" discriminator field.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query (natural language or code fragment)" },
                "mode": { "type": "string", "enum": ["code", "filename", "all"], "description": "code=AST-aware code search, filename=file name match, all=everything" },
                "output_mode": { "type": "string", "enum": ["expand", "locate"], "description": "expand=full AST block (default, for read/modify); locate=line-level grep-sized hits (~130 tok) for first-pass location" },
                "path": { "type": "string", "description": "Project root (auto-detected if omitted)" },
                "top_k": { "type": "integer", "description": "Number of results (default 2)", "default": 2 },
                "context_lines": { "type": "integer", "description": "Lines around each match in expand mode (default 0 = full block, >0 = window)", "default": 0 },
                "filename": { "type": "boolean", "description": "Shortcut: search file names only" },
                "all": { "type": "boolean", "description": "Shortcut: search all files" },
                "path_filter": { "type": "string", "description": "Restrict results to files whose path contains this substring (e.g. \"cache.rs\" or \"src/cache.rs\")" },
                "hint": { "type": "boolean", "description": "Set false to omit the _ai_instructions field from expand output (default true)", "default": true }
            },
            "required": ["query"]
        }),
    )
}

/// `file` 工具的 MCP Schema。
fn file_tool_schema() -> Tool {
    Tool::new(
        "file",
        "File search across ANY directory (no index needed). Searches filename + content via ripgrep (or built-in walker). Perfect for locating assets/configs/prompts in unindexed dirs like ~/Downloads.",
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Filename fragment or content term" },
                "path": { "type": "string", "description": "Directory to search (default: server cwd)" },
                "content": { "type": "boolean", "description": "Also match file content (default true). Set false for name-only.", "default": true },
                "name_only": { "type": "boolean", "description": "Alias for content=false (name match only)" },
                "top_k": { "type": "integer", "description": "Maximum results (default 20)", "default": 20 },
                "max_tokens": { "type": "integer", "description": "Cap output to roughly this many tokens (0 = unlimited)", "default": 0 }
            },
            "required": ["query"]
        }),
    )
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
        query.mode
    };

    let (key, _config) = match get_or_create_engine(&state, project_path.as_ref()).await {
        Ok(k) => k,
        Err(e) => {
            tracing::error!("Failed to get engine: {:?}", e);
            return Json(AiResponse::Expand(AiSearchOutput {
                query: query.query.clone(),
                mode: "expand",
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                aggregated: None,
                _ai_instructions: Some("error: failed to initialize search engine".to_string()),
            }));
        }
    };

    let mut engines = state.engines.lock().await;
    let pe = match engines.get_mut(&key) {
        Some(pe) => pe,
        None => {
            return Json(AiResponse::Expand(AiSearchOutput {
                query: query.query.clone(),
                mode: "expand",
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                aggregated: None,
                _ai_instructions: Some("error: engine not found".to_string()),
            }));
        }
    };

    let mut search_query = query.clone();
    search_query.mode = mode;

    // R1 (v0.4): auto-route when caller omitted output_mode — shared router
    // module, identical behavior to the CLI `ai` subcommand.
    if search_query.output_mode.is_none() && matches!(search_query.mode, SearchMode::Code) {
        let decision = crate::router::classify(&search_query.query);
        search_query.output_mode = Some(decision.output_mode);
        // Widen the candidate pool for symbol→locate; never shrink explicit top_k.
        search_query.top_k = search_query.top_k.max(decision.top_k);
        if search_query.max_tokens == 0 {
            search_query.max_tokens = decision.max_tokens;
        }
    }

    if search_query.top_k == 0 {
        search_query.top_k = 2;
    }
    // 3.0: expand default = full block (context_lines 0). Do NOT force 5.

    let context_lines = search_query.context_lines;
    let query_str = search_query.query.clone();
    let is_locate = matches!(search_query.output_mode, Some(crate::types::OutputMode::Locate));

    match pe.engine.search(search_query) {
        Ok(mut resp) => {
            if is_locate {
                Json(AiResponse::Locate(resp.into()))
            } else {
                postprocess::post_process_results(&mut resp, &query_str, context_lines);
                let mut out: AiSearchOutput = resp.into();
                // P1-2: caller may request `hint: false` to drop _ai_instructions (~200 tok).
                if !query.hint {
                    out._ai_instructions = None;
                }
                // R3: fold hot symbols (same shape on CLI and MCP paths).
                postprocess::aggregate_results(&mut out);
                Json(AiResponse::Expand(out))
            }
        }
        Err(e) => {
            tracing::error!("Search error: {:?}", e);
            Json(AiResponse::Expand(AiSearchOutput {
                query: query_str,
                mode: "expand",
                results: Vec::new(),
                total_hits: 0,
                search_time_ms: 0,
                multi_hop: None,
                aggregated: None,
                _ai_instructions: Some("error: search failed".to_string()),
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
                _ai_instructions: Some("error: file search failed".to_string()),
            });
        }
    };
    let elapsed = start.elapsed().as_millis() as u64;
    // P1-3: same token-budget truncation as the CLI `file` subcommand.
    let out_matches: Vec<crate::types::FileMatch> = if body.max_tokens > 0 {
        let mut total: usize = 0;
        let mut truncated = Vec::new();
        for m in matches {
            let tok = m.context.chars().count().div_ceil(2);
            if total + tok > body.max_tokens && !truncated.is_empty() {
                break;
            }
            total += tok;
            truncated.push(m);
        }
        truncated
    } else {
        matches
    };
    let mut out = AiFileOutput::from_matches(body.query, out_matches, elapsed);
    out.total_hits = out.matches.len();
    Json(out)
}

