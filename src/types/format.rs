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

const AI_HINT: &str = "I am STS-X 3.0, an AI-native unified code+file search engine. CLI: sts-x search \"q\" (code, --expand full block default | --locate line-level grep-sized), sts-x file \"q\" [--path DIR] (filename+content, zero-index via rg), sts-x search \"q\" -f (filename), sts-x search \"q\" --all (all files). Options: -c N (context lines, 0=full), -t N (results), --path DIR. MCP: POST {\"query\":\"...\",\"mode\":\"code|filename|all\",\"output_mode\":\"expand|locate\",\"top_k\":3} to /search; POST {\"query\":\"...\",\"path\":\"/abs/dir\",\"content\":true,\"top_k\":10} to /file. Response: abs_path+lines=read location, score=relevance. locate: each match is a line (grep-sized, ~130 tok) — need the full block? re-run with output_mode=expand on that symbol. expand: code=full block.";

#[derive(Debug, Serialize)]
pub struct AiSearchOutput {
    pub query: String,
    pub results: Vec<AiResultItem>,
    pub total_hits: usize,
    pub search_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_hop: Option<Vec<AiMultiHopStep>>,
    #[serde(rename = "_ai_instructions")]
    pub _ai_instructions: &'static str,
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
        AiSearchOutput {
            query: resp.query,
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
            _ai_instructions: AI_HINT,
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
}

#[derive(Debug, Serialize)]
pub struct AiLocateItem {
    pub file: String,
    pub line: usize,
    pub context: String,
    pub score: f32,
}

impl From<SearchResponse> for AiLocateOutput {
    fn from(resp: SearchResponse) -> Self {
        AiLocateOutput {
            query: resp.query,
            mode: "locate",
            matches: resp.locate_matches.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<LocateMatch> for AiLocateItem {
    fn from(m: LocateMatch) -> Self {
        AiLocateItem {
            file: m.file,
            line: m.line,
            context: m.context,
            score: m.score,
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
    #[serde(rename = "_ai_instructions")]
    pub _ai_instructions: &'static str,
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
            _ai_instructions: AI_HINT,
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
