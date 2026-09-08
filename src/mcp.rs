/*
 * mcp.rs — Native stdio MCP server (Rust, zero deps beyond std + engine).
 *
 * Why this exists (whitepaper v5.1 毕业体检 2026-07-31):
 *   - `sts-x serve` is HTTP-only; WorkBuddy/Claude Desktop expect stdio MCP.
 *   - The Python bridge worked but: (a) requires Python 3 — absent on stock
 *     Windows; (b) spawns a background HTTP process (port/orphan management);
 *     (c) breaks the "single binary, zero runtime deps" promise.
 *   - This module speaks MCP JSON-RPC over stdin/stdout and calls the search
 *     engine IN-PROCESS — no HTTP, no port, no extra process. Fully portable.
 *
 * Protocol (MCP 2025-03-26 subset): initialize / notifications/initialized /
 * ping / tools/list / tools/call. Line-delimited JSON-RPC on stdin/stdout.
 */

use crate::cache;
use crate::chunker::Chunker;
use crate::filesearch;
use crate::indexer::SearchIndex;
use crate::search::SearchEngine;
use crate::types::format::{AiFileOutput, AiLocateOutput, AiSearchOutput};
use crate::types::{IndexConfig, OutputMode, SearchMode, SearchQuery};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How many per-project engines an MCP session keeps resident.
///
/// One MCP session may legitimately serve several projects (the AI passes a
/// different `path` per call). Each engine holds an open tantivy index, so the
/// cache is bounded; on overflow we drop all of them (the next call rebuilds)
/// rather than letting a long-lived session grow without limit.
const MAX_CACHED_ENGINES: usize = 8;

/// Ensure a project root is indexed (mirrors server::get_or_create_engine).
///
/// `index_path_override` lets tests isolate their Tantivy index into a temp dir
/// so parallel tests don't fight over the same IndexWriter lock (LockBusy).
fn ensure_engine(root: &Path, index_path_override: Option<&Path>) -> anyhow::Result<SearchEngine> {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let index_path = match index_path_override {
        Some(p) => p.to_path_buf(),
        None => cache::index_dir_for(&canonical),
    };
    let config = IndexConfig {
        project_root: canonical.clone(),
        index_path: index_path.clone(),
        ..IndexConfig::default()
    };

    let tantivy_dir = index_path.join("tantivy");
    let semantic = crate::embed::semantic_requested();
    let mut needs_build =
        !tantivy_dir.join("meta.json").exists() || cache::is_index_stale(&index_path, &canonical);

    // P0-2: semantic 请求但现有索引无 embedding → 强制重建（否则语义静默失效）
    if !needs_build && semantic {
        if let Ok(idx) = SearchIndex::new(config.clone(), None) {
            if !idx.has_embeddings() {
                tracing::info!("Semantic requested but index has no embeddings; rebuilding");
                needs_build = true;
            }
        }
    }

    if needs_build {
        tracing::info!("Building index for {} ...", canonical.display());
        std::fs::create_dir_all(&index_path)?;
        let mut chunker = Chunker::new(&config.languages)?;
        let blocks = chunker.index_project(&canonical, &config)?;
        let embed_model = if semantic {
            crate::embed::maybe_load_model(&config)
        } else {
            None
        };
        let mut index = SearchIndex::new(config.clone(), embed_model)?;
        index.index_blocks(blocks)?;
        index.index_file_paths(&config)?;
        let embed_model = if semantic {
            crate::embed::maybe_load_model(&config)
        } else {
            None
        };
        Ok(SearchEngine::new(Arc::new(index), embed_model))
    } else {
        let index = SearchIndex::new(config.clone(), None)?;
        let embed_model = if semantic {
            crate::embed::maybe_load_model(&config)
        } else {
            None
        };
        Ok(SearchEngine::new(Arc::new(index), embed_model))
    }
}

/// Search engine cached per project root (simple, single-threaded stdio loop).
struct McpEngine {
    root: PathBuf,
    engine: SearchEngine,
}

impl McpEngine {
    fn for_path(root: &Path) -> anyhow::Result<Self> {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let engine = ensure_engine(&canonical, None)?;
        Ok(Self {
            root: canonical,
            engine,
        })
    }

    #[cfg(test)]
    fn for_path_with_index(root: &Path, index_path: &Path) -> anyhow::Result<Self> {
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        std::fs::create_dir_all(index_path).ok();
        let engine = ensure_engine(&canonical, Some(index_path))?;
        Ok(Self {
            root: canonical,
            engine,
        })
    }

    fn search(&mut self, args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        // Build SearchQuery from arbitrary JSON arguments (serde permissive).
        let mut q: SearchQuery = serde_json::from_value(args.clone()).unwrap_or_default();
        if q.path.is_none() {
            q.path = Some(self.root.clone());
        }
        // Apply output_mode routing like server handle_search: auto-route when absent.
        if q.output_mode.is_none() && matches!(q.mode, SearchMode::Code) {
            let decision = crate::router::classify(&q.query);
            q.output_mode = Some(decision.output_mode);
            q.top_k = q.top_k.max(decision.top_k);
            if q.max_tokens == 0 {
                q.max_tokens = decision.max_tokens;
            }
        }
        if q.top_k == 0 {
            q.top_k = 2;
        }

        let is_locate = matches!(q.output_mode, Some(OutputMode::Locate));
        let query_str = q.query.clone();
        let context_lines = q.context_lines;
        let mut resp = self.engine.search(q)?;

        if is_locate {
            Ok(serde_json::to_value(AiLocateOutput::from(resp))?)
        } else {
            crate::postprocess::post_process_results(&mut resp, &query_str, context_lines);
            let mut out: AiSearchOutput = resp.into();
            crate::postprocess::aggregate_results(&mut out);
            Ok(serde_json::to_value(out)?)
        }
    }

    fn file(&self, args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let name_only = args
            .get("name_only")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || !args
                .get("content")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let dir = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let max_tokens = args.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(0) as usize;

        let start = std::time::Instant::now();
        let matches = filesearch::search_files(&query, &dir, name_only, top_k, true)?;
        let elapsed = start.elapsed().as_millis() as u64;

        let out_matches: Vec<crate::types::FileMatch> = if max_tokens > 0 {
            let mut total = 0usize;
            let mut kept = Vec::new();
            for m in matches {
                let tok = m.context.chars().count().div_ceil(2);
                if total + tok > max_tokens && !kept.is_empty() {
                    break;
                }
                total += tok;
                kept.push(m);
            }
            kept
        } else {
            matches
        };
        let mut out = AiFileOutput::from_matches(query, out_matches, elapsed);
        out.total_hits = out.matches.len();
        Ok(serde_json::to_value(out)?)
    }

    fn glob(&self, args: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        let patterns = args
            .get("patterns")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let dir = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let max_tokens = args
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(500) as usize;
        let sort_recent = args
            .get("sort_recent")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let git_aware = args
            .get("git_aware")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let no_ignore = args
            .get("no_ignore")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let req = crate::globsearch::GlobRequest {
            patterns: patterns
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            dir,
            top_k,
            max_tokens,
            sort_recent,
            git_aware,
            no_ignore,
        };
        let start = std::time::Instant::now();
        let result = crate::globsearch::run_glob(&req)?;
        let elapsed = start.elapsed().as_millis() as u64;
        let out = crate::types::format::AiGlobOutput::from_result(patterns, result, elapsed);
        Ok(serde_json::to_value(out)?)
    }
}

fn tools_list() -> serde_json::Value {
    serde_json::json!({
        "tools": [
            {
                "name": "search",
                "description": "Unified code search (STS-X 3.3.3). BM25 over AST blocks, auto-indexes if needed, supports multi-project via path. Zero-hit AUTO-RETRY: 0 命中自动按英文同义词(本地词典)→符号猜测→file 内容兜底重试，单次调用即出最佳结果。Omit output_mode to AUTO-ROUTE: symbol-like query→locate (grep-sized, cheap), natural language→expand (full blocks, token-budgeted). Response carries a \"mode\" discriminator field. Semantic recall available via STX_SEMANTIC=1 / semantic field (Chinese NL → English code, e.g. 缓存目录在哪里 → cache.rs).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query (natural language or code fragment)" },
                        "mode": { "type": "string", "enum": ["code", "filename", "all"], "description": "code=AST-aware code search, filename=file name match, all=everything" },
                        "output_mode": { "type": "string", "enum": ["expand", "locate"], "description": "expand=full AST block (default, for read/modify); locate=line-level grep-sized hits (~130 tok) for first-pass location" },
                        "path": { "type": "string", "description": "Project root (auto-detected if omitted)" },
                        "top_k": { "type": "integer", "description": "Number of results (default 2)", "default": 2 },
                        "context_lines": { "type": "integer", "description": "Lines around each match in expand mode (default 0 = full block)", "default": 0 },
                        "path_filter": { "type": "string", "description": "Restrict results to files whose path contains this substring" },
                        "hint": { "type": "boolean", "description": "Set false to omit _ai_instructions (default true)" },
                        "filename": { "type": "boolean", "description": "Shortcut: search file names only" },
                        "all": { "type": "boolean", "description": "Shortcut: search all files" }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "file",
                "description": "File search across ANY directory (no index needed). Searches filename + content via ripgrep (or built-in walker).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Filename fragment or content term" },
                        "path": { "type": "string", "description": "Directory to search (default: cwd)" },
                        "content": { "type": "boolean", "description": "Also match file content (default true)", "default": true },
                        "name_only": { "type": "boolean", "description": "Name match only" },
                        "top_k": { "type": "integer", "description": "Maximum results (default 20)", "default": 20 },
                        "max_tokens": { "type": "integer", "description": "Cap output tokens (0 = unlimited)", "default": 0 }
                    },
                    "required": ["query"]
                }
            },
            {
                "name": "glob",
                "description": "List files matching a glob pattern, ranked for AI. Returns truncated results with total_hits and omitted count. Use this instead of raw file listing when discovering files by pattern (e.g. '**/*.rs', '*.toml,*.lock'). Honors .gitignore by default; pass no_ignore=true to include ignored files.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "patterns": { "type": "string", "description": "Glob pattern(s), comma-separated, e.g. '**/*.rs' or '*.toml,*.lock'" },
                        "path": { "type": "string", "description": "Directory to search (default: cwd)" },
                        "top_k": { "type": "integer", "description": "Maximum results (default 20)", "default": 20 },
                        "max_tokens": { "type": "integer", "description": "Cap output tokens (0 = unlimited, default 500)", "default": 500 },
                        "sort_recent": { "type": "boolean", "description": "Rank most-recently-modified first", "default": false },
                        "git_aware": { "type": "boolean", "description": "Rank git-modified/untracked files first", "default": false },
                        "no_ignore": { "type": "boolean", "description": "Do not respect .gitignore", "default": false }
                    },
                    "required": ["patterns"]
                }
            }
        ]
    })
}

/// Entry: `sts-x mcp [--path DIR]`. Reads JSON-RPC from stdin, writes to stdout.
pub fn run(default_root: &Path) -> anyhow::Result<()> {
    // Per-project engine registry, keyed by canonical project root.
    //
    // Previously a single engine was built from the FIRST request's root and
    // reused forever. Since `SearchEngine::search` ignores `SearchQuery::path`,
    // every later call with a different `path` silently returned hits from the
    // wrong project — the worst kind of bug (plausible output, wrong source).
    // Keying by root makes `path` actually work across projects/disks.
    let mut engines: HashMap<PathBuf, McpEngine> = HashMap::new();
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let req: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let _ = writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": null,
                        "error": {"code": -32700, "message": format!("parse error: {e}")}
                    })
                );
                let _ = stdout.flush();
                continue;
            }
        };

        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let id = req.get("id").cloned();

        let response: Option<serde_json::Value> = match method {
            "initialize" => Some(serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "sts-x", "version": env!("CARGO_PKG_VERSION")}
                }
            })),
            "notifications/initialized" | "notifications/cancelled" => None,
            "ping" => Some(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {}})),
            "tools/list" => {
                Some(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": tools_list()}))
            }
            "tools/call" => {
                let name = req
                    .pointer("/params/name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let args = req
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));
                // Resolve project root: explicit args.path > --path default > cwd.
                let raw_root = args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_root.to_path_buf());
                // Canonicalize so `/path`, `/path/` and a symlinked variant all
                // map to the same engine instead of building duplicates.
                let root = raw_root
                    .canonicalize()
                    .unwrap_or_else(|_| raw_root.to_path_buf());
                let result = (|| -> anyhow::Result<serde_json::Value> {
                    if !engines.contains_key(&root) {
                        if engines.len() >= MAX_CACHED_ENGINES {
                            engines.clear();
                        }
                        tracing::info!("mcp: loading engine for {}", root.display());
                        engines.insert(root.clone(), McpEngine::for_path(&root)?);
                    }
                    let eng = engines.get_mut(&root).unwrap();
                    match name {
                        "search" => eng.search(&args),
                        "file" => eng.file(&args),
                        "glob" => eng.glob(&args),
                        _ => anyhow::bail!("unknown tool: {name}"),
                    }
                })();
                // Record activity so the idle GC scanner waits for a quiet gap.
                crate::gc::touch_activity();
                match result {
                    Ok(data) => Some(serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "content": [{"type": "text", "text": data.to_string()}],
                            "isError": false
                        }
                    })),
                    Err(e) => Some(serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "error": {"code": -32603, "message": e.to_string()}
                    })),
                }
            }
            _ => Some(serde_json::json!({
                "jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("method not implemented: {method}")}
            })),
        };

        if let Some(resp) = response {
            let _ = writeln!(stdout, "{resp}");
            let _ = stdout.flush();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_has_input_schema_camel_case() {
        // MCP 协议字段必须 inputSchema（驼峰）—— WorkBuddy 严格校验
        let v = tools_list();
        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3);
        for t in tools {
            assert!(t.get("inputSchema").is_some(), "missing inputSchema: {t}");
            assert!(t.get("input_schema").is_none(), "must NOT be snake_case");
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        assert_eq!(tools[0]["name"], "search");
        assert_eq!(tools[1]["name"], "file");
        assert_eq!(tools[2]["name"], "glob");
    }

    #[test]
    fn glob_tool_requires_patterns_and_is_camel_case() {
        let v = tools_list();
        let tools = v["tools"].as_array().unwrap();
        let glob = tools
            .iter()
            .find(|t| t["name"] == "glob")
            .expect("glob tool present");
        assert_eq!(glob["inputSchema"]["type"], "object");
        let props = glob["inputSchema"]["properties"].as_object().unwrap();
        // All field names must be camelCase (MCP strict validation).
        for key in [
            "patterns",
            "path",
            "top_k",
            "max_tokens",
            "sort_recent",
            "git_aware",
            "no_ignore",
        ] {
            assert!(props.contains_key(key), "missing glob prop: {key}");
        }
        let required = glob["inputSchema"]["required"].as_array().unwrap();
        assert!(
            required.contains(&serde_json::json!("patterns")),
            "patterns must be required"
        );
    }

    #[test]
    fn engine_search_auto_routes_symbol_to_locate() {
        // 无 output_mode 时符号查询自动走 locate（与 CLI ai / HTTP server 一致）
        // 用独立临时索引目录，避免并行测试争抢同一 Tantivy 写锁（LockBusy）
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let idx = std::env::temp_dir().join(format!("stsx-mcp-test-{}", std::process::id()));
        let mut eng = McpEngine::for_path_with_index(root, &idx).expect("engine");
        let args = serde_json::json!({"query": "McpServer", "top_k": 2});
        let out = eng.search(&args).expect("search");
        assert_eq!(out["mode"], "locate");
    }

    #[test]
    fn engine_search_chinese_hint_on_zero_hits() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let idx = std::env::temp_dir().join(format!("stsx-mcp-test-{}-cn", std::process::id()));
        let mut eng = McpEngine::for_path_with_index(root, &idx).expect("engine");
        // 动态拼接避免源码含完整查询串（否则测试文件自身会被索引命中）
        let q = format!("zzqq_{}x9", "量子纠缠不存在");
        let args = serde_json::json!({"query": q, "output_mode": "locate"});
        let out = eng.search(&args).expect("search");
        let matches = out["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "should be 0 hits, got {matches:?}");
        // locate 0 命中时应有 hint 自救（v5.1 契约）
        let hint = out["hint"].as_str().unwrap_or("");
        assert!(
            hint.contains("换英文关键词") || hint.contains("换英文"),
            "hint={hint}"
        );
    }
}
