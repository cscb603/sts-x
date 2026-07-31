/*
 * format.rs
 * Project: sts-x
 * Description: AI-optimized output format specification and serialization
 *
 * Design principles for AI consumption:
 * - Flat, minimal structure (less nesting = easier parsing, fewer tokens)
 * - Precise line numbers with highlight_lines (no need to grep again)
 * - Context window controlled by context_lines (default 5 lines) — avoids token bloat
 * - Absolute path for direct file read/edit operations
 * - _ai_instructions field: always present, tells AI how to use results
 */

use crate::types::{FileMatch, LocateMatch, SearchResponse, SearchResult};
use serde::Serialize;

pub const AI_HINT: &str = concat!(
    "I am STS-X ",
    env!("CARGO_PKG_VERSION"),
    ", an AI-native unified code+file search engine. CLI: sts-x search \"q\" (code, --expand full block default | --locate line-level grep-sized), sts-x file \"q\" [--path DIR] (filename+content, zero-index via rg), sts-x search \"q\" -f (filename), sts-x search \"q\" --all (all files). Options: -c N (context lines, 0=full), -t N (results), --path DIR. MCP: POST {\"query\":\"...\",\"mode\":\"code|filename|all\",\"output_mode\":\"expand|locate\",\"top_k\":3} to /search; POST {\"query\":\"...\",\"path\":\"/abs/dir\",\"content\":true,\"top_k\":10} to /file. Response: abs_path+lines=read location, score=relevance. locate: each match is a line (grep-sized, ~130 tok) — need the full block? re-run with output_mode=expand on that symbol. expand: code=full block."
);

/// True for CJK ideographs / kana / hangul. Mirrors `indexer::is_cjk` (kept
/// local so format.rs has zero coupling to the indexer module).
fn is_cjk(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4dbf}' | // CJK Ext A
        '\u{4e00}'..='\u{9fff}' | // CJK Unified
        '\u{3040}'..='\u{30ff}' | // Hiragana / Katakana
        '\u{ac00}'..='\u{d7af}'   // Hangul
    )
}

/// v5.1 (P0-1): dynamic AI guidance — the zero-hit self-rescue contract.
/// `hits == 0` → language-matched rescue suggestions (Chinese query → Chinese
/// advice, ASCII → English) so the AI can recover without a dead end; `hits > 0`
/// → the normal usage guide (`AI_HINT`). `mode` ("expand" | "locate" | "file")
/// is reserved for future per-mode guidance.
pub fn build_hint(query: &str, hits: usize, _mode: &str) -> String {
    if hits > 0 {
        return AI_HINT.to_string();
    }
    if query.chars().any(is_cjk) {
        format!(
            "未找到直接匹配「{}」。建议：① 换英文关键词（如 cache 对应缓存）；② 用符号/函数名（如 cache_root）；③ 用 --path-filter 限定文件；④ 用 sts-x file 搜文件名",
            query
        )
    } else {
        format!(
            "No direct match for \"{}\". Try: ① a symbol name (e.g. cache_root); ② a file name; ③ broader terms; ④ --path-filter",
            query
        )
    }
}

#[derive(Debug, Serialize)]
pub struct AiSearchOutput {
    pub query: String,
    /// R1 (v0.4): discriminator field ("expand") so AI can deterministically
    /// parse CLI and MCP responses the same way (locate output carries "locate").
    pub mode: &'static str,
    pub results: Vec<AiResultItem>,
    pub total_hits: usize,
    pub search_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_hop: Option<Vec<AiMultiHopStep>>,
    /// R3 (v0.4): hot-symbol aggregation. When one symbol name matches blocks
    /// in many files, the flat listing is folded into per-symbol groups
    /// (`file_count` + top-3 by score) — same shape on CLI and MCP paths.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregated: Option<Vec<AiAggregateGroup>>,
    #[serde(rename = "_ai_instructions", skip_serializing_if = "Option::is_none")]
    pub _ai_instructions: Option<String>,
}

/// R3: one aggregated group — a symbol that matched across multiple blocks/files.
#[derive(Debug, Serialize)]
pub struct AiAggregateGroup {
    pub symbol: String,
    pub file_count: usize,
    /// Total matched blocks folded into this group (before top-3 cut).
    pub match_count: usize,
    pub top: Vec<AiResultItem>,
}

#[derive(Debug, Serialize)]
pub struct AiResultItem {
    pub score: f32,
    pub path: String,
    pub abs_path: String,
    pub lines: (usize, usize),
    pub highlight_lines: Vec<usize>,
    pub kind: String,
    pub name: String,
    pub signature: String,
    pub summary: String,
    pub code: String,
    pub language: String,
}

#[derive(Debug, Serialize)]
pub struct AiMultiHopStep {
    pub sub_query: String,
    pub results: Vec<AiResultItem>,
    pub search_time_ms: u64,
}

impl From<SearchResponse> for AiSearchOutput {
    fn from(resp: SearchResponse) -> Self {
        // v5.1 (P0-1): compute dynamic guidance before moving fields out.
        let hint = build_hint(&resp.query, resp.total_hits, "expand");
        AiSearchOutput {
            query: resp.query,
            mode: "expand",
            results: resp.results.into_iter().map(Into::into).collect(),
            total_hits: resp.total_hits,
            search_time_ms: resp.search_time_ms,
            multi_hop: resp.multi_hop.map(|steps| {
                steps
                    .into_iter()
                    .map(|s| AiMultiHopStep {
                        sub_query: s.sub_query,
                        results: s.results.into_iter().map(Into::into).collect(),
                        search_time_ms: s.search_time_ms,
                    })
                    .collect()
            }),
            aggregated: None, // filled by postprocess::aggregate_results (R3)
            _ai_instructions: Some(hint),
        }
    }
}

impl From<SearchResult> for AiResultItem {
    fn from(r: SearchResult) -> Self {
        let b = r.block;
        AiResultItem {
            score: r.score,
            path: b.path.display().to_string(),
            abs_path: b.abs_path.display().to_string(),
            lines: (b.start_line, b.end_line),
            highlight_lines: r.highlight_lines,
            kind: format!("{:?}", b.kind).to_lowercase(),
            name: b.name,
            signature: b.signature,
            summary: b.doc_comment,
            code: b.code,
            language: b.language,
        }
    }
}

// ─── 3.0 locate-mode output (grep-sized line hits) ───────────────
// Deliberately minimal (file/line/context/score) — no abs_path, no
// _ai_instructions — so a locate call stays ~130 tok, far below the
// grep+Read flow. The AI expands a symbol via --expand when it needs more.
#[derive(Debug, Serialize)]
pub struct AiLocateOutput {
    pub query: String,
    pub mode: &'static str,
    pub matches: Vec<AiLocateItem>,
    /// v5.1 (P0-2): zero-hit rescue hint — serialized ONLY when matches is
    /// empty, so a normal locate call stays ~130 tok (grep-sized).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AiLocateItem {
    pub file: String,
    /// Absolute path so the AI can Read the file directly (whitepaper §7 P0-2).
    pub abs_path: String,
    pub line: usize,
    pub context: String,
    pub score: f32,
    /// Symbol name of the containing AST block ("" for plain file hits).
    pub name: String,
}

impl From<SearchResponse> for AiLocateOutput {
    fn from(resp: SearchResponse) -> Self {
        AiLocateOutput {
            query: resp.query,
            mode: "locate",
            matches: resp.locate_matches.into_iter().map(Into::into).collect(),
            hint: resp.hint,
        }
    }
}

impl From<LocateMatch> for AiLocateItem {
    fn from(m: LocateMatch) -> Self {
        AiLocateItem {
            file: m.file,
            abs_path: m.abs_path,
            line: m.line,
            context: m.context,
            score: m.score,
            name: m.name,
        }
    }
}

// ─── 3.0 file-mode output (filename + content, zero-index) ─────────
#[derive(Debug, Serialize)]
pub struct AiFileOutput {
    pub query: String,
    pub mode: &'static str,
    pub matches: Vec<AiFileItem>,
    pub total_hits: usize,
    pub search_time_ms: u64,
    #[serde(rename = "_ai_instructions", skip_serializing_if = "Option::is_none")]
    pub _ai_instructions: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AiFileItem {
    pub path: String,
    pub abs_path: String,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
    pub matched_by: String,
    pub line: usize,
    pub context: String,
}

impl AiFileOutput {
    pub fn from_matches(query: String, matches: Vec<FileMatch>, search_time_ms: u64) -> Self {
        // v5.1 (P0-1): dynamic guidance for the file path too.
        let hint = build_hint(&query, matches.len(), "file");
        AiFileOutput {
            query,
            mode: "file",
            matches: matches
                .into_iter()
                .map(|m| AiFileItem {
                    path: m.path,
                    abs_path: m.abs_path,
                    size: m.size,
                    mtime: m.mtime,
                    is_dir: m.is_dir,
                    matched_by: m.matched_by,
                    line: m.line,
                    context: m.context,
                })
                .collect(),
            total_hits: 0, // filled by caller (matches are built already)
            search_time_ms,
            _ai_instructions: Some(hint),
        }
    }
}

pub fn format_human_readable(resp: &SearchResponse) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "STS-X Search Results\n  Query: {}\n  Hits:  {} ({}ms)\n\n",
        resp.query, resp.total_hits, resp.search_time_ms,
    ));

    for (i, result) in resp.results.iter().enumerate() {
        let b = &result.block;
        let kind_str = match b.kind {
            crate::types::BlockKind::Function => "fn",
            crate::types::BlockKind::Method => "fn",
            crate::types::BlockKind::Class => "class",
            crate::types::BlockKind::Struct => "struct",
            crate::types::BlockKind::Enum => "enum",
            crate::types::BlockKind::Trait => "trait",
            crate::types::BlockKind::Impl => "impl",
            crate::types::BlockKind::Module => "mod",
            crate::types::BlockKind::Interface => "trait",
            crate::types::BlockKind::Type => "type",
            crate::types::BlockKind::Block => "file",
        };
        output.push_str(&format!(
            "[{}/{}] {:.0}%  {}:{}{}\n  {} {}\n",
            i + 1,
            resp.results.len(),
            result.score * 100.0,
            b.path.display(),
            b.start_line,
            if !result.highlight_lines.is_empty() {
                format!("  [matches: L{}]", result.highlight_lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join(", L"))
            } else {
                String::new()
            },
            kind_str,
            b.signature,
        ));
        if !b.doc_comment.is_empty() {
            output.push_str(&format!("  /// {}\n", b.doc_comment));
        }
        output.push_str(&format!("{}\n\n", b.code));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::LocateMatch;

    #[test]
    fn locate_item_carries_abs_path_and_name() {
        let m = LocateMatch {
            score: 0.9,
            file: "src/mcp/mod.rs".to_string(),
            abs_path: "/abs/libs/core_lib/src/mcp/mod.rs".to_string(),
            line: 64,
            context: "pub struct McpServer {".to_string(),
            kind: "struct".to_string(),
            name: "McpServer".to_string(),
        };
        let item: AiLocateItem = m.into();
        assert_eq!(item.abs_path, "/abs/libs/core_lib/src/mcp/mod.rs");
        assert_eq!(item.name, "McpServer");
        assert_eq!(item.line, 64);
    }

    #[test]
    fn locate_output_serializes_abs_path_field() {
        let m = LocateMatch {
            score: 1.0,
            file: "src/mcp/mod.rs".to_string(),
            abs_path: "/abs/libs/core_lib/src/mcp/mod.rs".to_string(),
            line: 64,
            context: "pub struct McpServer {".to_string(),
            kind: "struct".to_string(),
            name: "McpServer".to_string(),
        };
        let item: AiLocateItem = m.into();
        let json = serde_json::to_value(&item).unwrap();
        // Whitepaper §7 P0-2: AI must be able to Read the file from locate output.
        assert_eq!(json["abs_path"], "/abs/libs/core_lib/src/mcp/mod.rs");
        assert!(json.get("abs_path").is_some());
        assert!(json.get("name").is_some());
    }

    // ── v5.1 (P0-1 / P0-2): zero-hit self-rescue guidance ──────────────

    #[test]
    fn build_hint_zero_hits_cjk_returns_chinese() {
        let hint = build_hint("索引过期", 0, "expand");
        // Whitepaper §3 P0: Chinese query → Chinese rescue advice.
        assert!(hint.contains("未找到直接匹配「索引过期」"), "got: {hint}");
        assert!(hint.contains("换英文关键词"), "got: {hint}");
        assert!(hint.contains("符号/函数名"), "got: {hint}");
    }

    #[test]
    fn build_hint_zero_hits_ascii_returns_english() {
        let hint = build_hint("cache_root_xyz", 0, "expand");
        assert!(hint.contains("No direct match"), "got: {hint}");
        assert!(hint.contains("symbol name"), "got: {hint}");
    }

    #[test]
    fn build_hint_with_hits_returns_usage_guide() {
        // hits > 0 → the normal usage guide (not the rescue template).
        assert_eq!(build_hint("缓存", 3, "expand"), AI_HINT);
        assert_eq!(build_hint("cache_root", 1, "locate"), AI_HINT);
    }

    #[test]
    fn locate_hint_none_is_omitted_from_json() {
        let resp = SearchResponse {
            query: "McpServer".to_string(),
            results: Vec::new(),
            total_hits: 0,
            search_time_ms: 1,
            multi_hop: None,
            locate_matches: Vec::new(),
            hint: None,
        };
        let out: AiLocateOutput = resp.into();
        let json = serde_json::to_value(&out).unwrap();
        // Protocol compat (whitepaper §5): optional field must be absent.
        assert!(json.get("hint").is_none());
    }

    #[test]
    fn locate_hint_some_is_serialized_and_passthrough() {
        let resp = SearchResponse {
            query: "索引过期".to_string(),
            results: Vec::new(),
            total_hits: 0,
            search_time_ms: 1,
            multi_hop: None,
            locate_matches: Vec::new(),
            hint: Some(build_hint("索引过期", 0, "locate")),
        };
        let out: AiLocateOutput = resp.into();
        let json = serde_json::to_value(&out).unwrap();
        let h = json.get("hint").expect("hint must be serialized");
        assert!(h.as_str().unwrap().contains("换英文关键词"));
        assert_eq!(json["mode"], "locate");
    }
}
