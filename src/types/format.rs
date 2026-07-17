/*
 * format.rs
 * Project: sts-x
 * Description: AI-optimized output format specification and serialization
 */

use crate::types::{SearchResponse, SearchResult};
use serde::Serialize;

/// AI-optimized search output format.
///
/// This format is designed for LLM consumption:
/// - Minimal, flat structure (AI prefers flat over nested)
/// - Self-contained results (each result has all context)
/// - Rich metadata (paths, signatures, summaries)
/// - Search time for AI to assess freshness
///
/// # JSON Output Example for AI
///
/// ```json
/// {
///   "query": "how to verify user token",
///   "results": [
///     {
///       "score": 0.97,
///       "path": "src/auth/jwt.rs",
///       "abs_path": "/project/src/auth/jwt.rs",
///       "lines": [42, 78],
///       "kind": "function",
///       "name": "verify_token",
///       "signature": "fn verify_token(token: &str) -> Result<Claims>",
///       "summary": "验证 JWT token 签名和过期时间",
///       "code": "pub fn verify_token(token: &str) -> Result<Claims> {\n    ...\n}",
///       "language": "rust"
///     }
///   ],
///   "total_hits": 5,
///   "search_time_ms": 35
/// }
/// ```
///
/// # AI Usage Guidelines (documented for AI consumption)
///
/// 1. The `score` field represents relevance (0-1). Focus on score > 0.6.
/// 2. `code` field contains the full source code of the matched block.
/// 3. `signature` gives you the exact function/type signature.
/// 4. Use `abs_path` for file operations (read, edit).
/// 5. When the result contains `imports`, use them to understand dependencies.
/// 6. `multi_hop` results contain decomposed sub-queries for complex questions.
#[derive(Debug, Serialize)]
pub struct AiSearchOutput {
    pub query: String,
    pub results: Vec<AiResultItem>,
    pub total_hits: usize,
    pub search_time_ms: u64,
    pub multi_hop: Option<Vec<AiMultiHopStep>>,

    /// Instructions for AI consumption
    #[serde(skip_serializing)]
    pub _ai_instructions: &'static str,
}

/// Compressed result item optimized for token efficiency
#[derive(Debug, Serialize)]
pub struct AiResultItem {
    pub score: f32,
    pub path: String,
    pub abs_path: String,
    pub lines: (usize, usize),
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

/// Convert SearchResponse to AI-optimized output
impl From<SearchResponse> for AiSearchOutput {
    fn from(resp: SearchResponse) -> Self {
        AiSearchOutput {
            _ai_instructions: AI_INSTRUCTIONS,
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
            kind: format!("{:?}", b.kind).to_lowercase(),
            name: b.name,
            signature: b.signature,
            summary: b.doc_comment,
            code: b.code,
            language: b.language,
        }
    }
}

/// Human-optimized text output
pub fn format_human_readable(resp: &SearchResponse) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "═══ Search Results ═══════════════════════════\n\
         Query: {}\n\
         Hits: {} ({}ms)\n\n",
        resp.query, resp.total_hits, resp.search_time_ms,
    ));

    for (i, result) in resp.results.iter().enumerate() {
        let b = &result.block;
        output.push_str(&format!(
            "── [{}/{}] {:.0}% ────────────────────────────\n\
             {}:{} - {}\n\
             {} {}\n\
             {}\n\n",
            i + 1,
            resp.results.len(),
            result.score * 100.0,
            b.path.display(),
            b.start_line,
            b.name,
            match b.kind {
                crate::types::BlockKind::Function => "fn",
                crate::types::BlockKind::Class => "class",
                crate::types::BlockKind::Struct => "struct",
                crate::types::BlockKind::Enum => "enum",
                crate::types::BlockKind::Trait => "trait",
                _ => "block",
            },
            b.signature,
            if b.doc_comment.is_empty() {
                String::new()
            } else {
                format!("  {}\n", b.doc_comment)
            },
        ));
    }

    output
}

const AI_INSTRUCTIONS: &str = r#"
AI Consumption Instructions:
- score > 0.9: Highly relevant. Use directly.
- score > 0.7: Relevant. Check if context is sufficient.
- score > 0.5: Possibly relevant. Review before using.
- score < 0.5: Low confidence. Use only as supplementary context.
- When using multi_hop results, each sub_query is a decomposed aspect of the original question.
- The `code` field contains the full source code of the matched block.
"#;
