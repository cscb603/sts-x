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

use crate::chunker::Chunker;
use crate::cache;
use crate::filesearch;
use crate::indexer::SearchIndex;
use crate::search::SearchEngine;
use crate::types::{IndexConfig, OutputMode, SearchMode, SearchQuery};
use crate::types::format::{AiFileOutput, AiLocateOutput, AiSearchOutput};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Ensure a project root is indexed (mirrors server::get_or_create_engine).
fn ensure_engine(root: &Path) -> anyhow::Result<SearchEngine> {
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let index_path = cache::index_dir_for(&canonical);
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
        std::fs::create_dir_all(&index_path)?;
        let mut chunker = Chunker::new(&config.languages)?;
        let blocks = chunker.index_project(&canonical, &config)?;
        let mut index = SearchIndex::new(config.clone(), None)?;
        index.index_blocks(blocks)?;
        index.index_file_paths(&config)?;
        Ok(SearchEngine::new(Arc::new(index), None))
    } else {
        let index = SearchIndex::new(config.clone(), None)?;
        Ok(SearchEngine::new(Arc::new(index), None))
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
        let engine = ensure_engine(&canonical)?;
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
        let name_only = args.get("name_only").and_then(|v| v.as_bool()).unwrap_or(false)
            || !args.get("content").and_then(|v| v.as_bool()).unwrap_or(true);
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let dir = args.get("path")
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
}

fn tools_list() -> serde_json::Value {
    serde_json::json!({
        "tools": [
            {
                "name": "search",
                "description": "Unified code search (STS-X 3.2.0). BM25 over AST blocks, auto-indexes if needed, supports multi-project via path. Omit output_mode to AUTO-ROUTE: symbol-like query→locate (grep-sized, cheap), natural language→expand (full blocks, token-budgeted). Response carries a \"mode\" discriminator field.",
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
            }
        ]
    })
}

/// Entry: `sts-x mcp [--path DIR]`. Reads JSON-RPC from stdin, writes to stdout.
pub fn run(default_root: &Path) -> anyhow::Result<()> {
    let mut engine_holder: Option<McpEngine> = None;
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
                let _ = writeln!(stdout, "{}", serde_json::json!({
                    "jsonrpc": "2.0", "id": null,
                    "error": {"code": -32700, "message": format!("parse error: {e}")}
                }));
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
            "tools/list" => Some(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": tools_list()})),
            "tools/call" => {
                let name = req.pointer("/params/name").and_then(|v| v.as_str()).unwrap_or("");
                let args = req.pointer("/params/arguments").cloned().unwrap_or(serde_json::json!({}));
                // Resolve project root: explicit args.path > --path default > cwd.
                let root = args.get("path")
                    .and_then(|v| v.as_str())
                    .map(PathBuf::from)
                    .or_else(|| Some(default_root.to_path_buf()));
                let result = (|| -> anyhow::Result<serde_json::Value> {
                    if engine_holder.is_none() {
                        engine_holder = Some(McpEngine::for_path(root.as_deref().unwrap())?);
                    }
                    let eng = engine_holder.as_mut().unwrap();
                    match name {
                        "search" => eng.search(&args),
                        "file" => eng.file(&args),
                        _ => anyhow::bail!("unknown tool: {name}"),
                    }
                })();
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
        assert_eq!(tools.len(), 2);
        for t in tools {
            assert!(t.get("inputSchema").is_some(), "missing inputSchema: {t}");
            assert!(t.get("input_schema").is_none(), "must NOT be snake_case");
            assert_eq!(t["inputSchema"]["type"], "object");
        }
        assert_eq!(tools[0]["name"], "search");
        assert_eq!(tools[1]["name"], "file");
    }

    #[test]
    fn engine_search_auto_routes_symbol_to_locate() {
        // 无 output_mode 时符号查询自动走 locate（与 CLI ai / HTTP server 一致）
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut eng = McpEngine::for_path(root).expect("engine");
        let args = serde_json::json!({"query": "McpServer", "top_k": 2});
        let out = eng.search(&args).expect("search");
        assert_eq!(out["mode"], "locate");
    }

    #[test]
    fn engine_search_chinese_hint_on_zero_hits() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut eng = McpEngine::for_path(root).expect("engine");
        // 动态拼接避免源码含完整查询串（否则测试文件自身会被索引命中）
        let q = format!("zzqq_{}x9", "量子纠缠不存在");
        let args = serde_json::json!({"query": q, "output_mode": "locate"});
        let out = eng.search(&args).expect("search");
        let matches = out["matches"].as_array().unwrap();
        assert!(matches.is_empty(), "should be 0 hits, got {matches:?}");
        // locate 0 命中时应有 hint 自救（v5.1 契约）
        let hint = out["hint"].as_str().unwrap_or("");
        assert!(hint.contains("换英文关键词") || hint.contains("换英文"), "hint={hint}");
    }
}
