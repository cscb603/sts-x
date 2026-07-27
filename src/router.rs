/*
 * router.rs
 * Project: sts-x
 * Description: Shared query router for the CLI `ai` subcommand and the MCP
 *              `search` tool (R1, whitepaper v0.4 §7.5).
 *
 * Both entry points call `classify(query)` so that an AI gets identical
 * routing behavior no matter whether it invokes sts-x as a subprocess (CLI)
 * or through the MCP HTTP interface:
 *   - symbol-like query  → Locate mode (grep-sized hits, ~130 tok) + higher top_k
 *   - natural language   → Expand mode (full AST blocks) + default token budget
 *
 * Pure cross-platform, zero extra dependencies (no regex crate: the
 * symbol check is a hand-rolled character scan).
 */

use crate::types::OutputMode;

/// Default token budget applied to auto-routed natural-language (expand) queries.
pub const DEFAULT_NL_MAX_TOKENS: usize = 1500;

/// Higher top_k for symbol → locate routing (locate output is line-sized, cheap).
pub const SYMBOL_TOP_K: usize = 5;

/// Routing decision shared by CLI `ai` and MCP `search`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RouteDecision {
    pub output_mode: OutputMode,
    pub top_k: usize,
    /// Suggested token budget (0 = unlimited; locate is already line-capped).
    pub max_tokens: usize,
}

/// Classify a query as symbol-like (→ locate) or natural language (→ expand).
///
/// Symbol heuristic: the whole query matches `^[A-Za-z_][A-Za-z0-9_:]*$`
/// (single token, identifier charset, `::` paths allowed) AND does not look
/// like a plain lowercase English word (single all-lowercase word without
/// `_`/`::`/inner uppercase is treated as natural language — e.g. "search"
/// is ambiguous, while "Cli", "run_search", "cache::detect" are symbols).
pub fn classify(query: &str) -> RouteDecision {
    if is_symbol_like(query) {
        RouteDecision {
            output_mode: OutputMode::Locate,
            top_k: SYMBOL_TOP_K,
            // locate output is already grep-sized/budget-capped per line
            max_tokens: 0,
        }
    } else {
        RouteDecision {
            output_mode: OutputMode::Expand,
            top_k: 2,
            max_tokens: DEFAULT_NL_MAX_TOKENS,
        }
    }
}

/// Public symbol check for the R2 fast path in `search_code_mode`
/// (exact chunk-name match before falling back to BM25).
pub fn is_symbol(query: &str) -> bool {
    is_symbol_like(query)
}

/// `^[A-Za-z_][A-Za-z0-9_:]*$` + "not a plain natural-language word".
fn is_symbol_like(query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() || q.contains(char::is_whitespace) {
        return false; // multi-word or empty → natural language
    }

    let mut chars = q.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false, // CJK / digit / punctuation start → NL
    }
    if !q.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':') {
        return false; // any non-identifier char (incl. CJK) → NL
    }

    // Plain single lowercase word (no '_', no "::", no inner uppercase) is
    // ambiguous English → treat as natural language.
    let has_code_shape = q.contains('_')
        || q.contains("::")
        || q.chars().any(|c| c.is_ascii_uppercase());
    has_code_shape
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::OutputMode;

    #[test]
    fn symbols_route_to_locate() {
        for q in ["Cli", "run_search", "AiSearchOutput", "cache::detect_project_root", "_private"] {
            assert_eq!(classify(q).output_mode, OutputMode::Locate, "query: {q}");
        }
    }

    #[test]
    fn natural_language_routes_to_expand() {
        for q in [
            "处理文件搜索的模块",
            "how does indexing work",
            "search",              // plain lowercase word → ambiguous → NL
            "3rd_party",           // digit start → NL
            "",
        ] {
            let d = classify(q);
            assert_eq!(d.output_mode, OutputMode::Expand, "query: {q}");
            assert_eq!(d.max_tokens, DEFAULT_NL_MAX_TOKENS);
        }
    }
}
