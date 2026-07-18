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

use crate::types::{SearchResponse, SearchResult};
use serde::Serialize;

const AI_HINT: &str = "I am STS-X, an AI-native code search engine. CLI: sts-x search \"q\" (code), sts-x search \"q\" -f (filename), sts-x search \"q\" --all (all files). Options: -c N (context lines, 0=full), -t N (results count), --path /dir (project root). MCP: POST {\"query\":\"...\",\"mode\":\"code|filename|all\",\"top_k\":3,\"context_lines\":5} to /search. Response: abs_path+lines=read location, highlight_lines=exact matches, score=relevance, code=truncated snippet.";

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
