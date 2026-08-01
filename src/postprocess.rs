/*
 * postprocess.rs
 * Project: sts-x
 * Description: Post-process search results for AI consumption
 *
 * - Computes highlight_lines (exact line numbers where query terms match)
 * - Truncates code to context window around the first match
 * - Adjusts start_line/end_line to reflect the snippet
 */

use crate::types::SearchResponse;
use crate::types::format::{AiAggregateGroup, AiResultItem, AiSearchOutput};

pub fn post_process_results(resp: &mut SearchResponse, query: &str, context_lines: usize) {
    if context_lines == 0 {
        for result in &mut resp.results {
            result.highlight_lines = find_matching_lines(&result.block.code, query, result.block.start_line);
        }
        return;
    }

    let query_terms: Vec<&str> = query
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .collect();

    if query_terms.is_empty() {
        return;
    }

    for result in &mut resp.results {
        if result.block.code.is_empty() {
            continue;
        }

        let lines: Vec<String> = result.block.code.lines().map(|l| l.to_string()).collect();
        if lines.is_empty() {
            continue;
        }

        let match_offset = lines.iter().position(|line| {
            let line_lower = line.to_lowercase();
            query_terms.iter().any(|term| line_lower.contains(&term.to_lowercase()))
        });

        let match_offset = match_offset.unwrap_or(0);

        let start_off = match_offset.saturating_sub(context_lines);
        let end_off = std::cmp::min(match_offset + context_lines + 1, lines.len());

        let abs_start = result.block.start_line + start_off;
        let abs_end = result.block.start_line + end_off - 1;

        let highlight: Vec<usize> = (start_off..end_off)
            .filter(|&i| {
                let line_lower = lines[i].to_lowercase();
                query_terms.iter().any(|term| line_lower.contains(&term.to_lowercase()))
            })
            .map(|i| result.block.start_line + i)
            .collect();

        let snippet = lines[start_off..end_off].join("\n");
        let code = if start_off > 0 && end_off < lines.len() {
            format!("// ... (+{} lines above)\n{}\n// ... ({} more lines)", start_off, snippet, lines.len().saturating_sub(end_off))
        } else if start_off > 0 {
            format!("// ... (+{} lines above)\n{}", start_off, snippet)
        } else if end_off < lines.len() {
            format!("{}\n// ... ({} more lines)", snippet, lines.len().saturating_sub(end_off))
        } else {
            snippet
        };

        result.block.code = code;
        result.block.start_line = abs_start;
        result.block.end_line = abs_end;
        result.highlight_lines = highlight;
    }
}

/// R3 (v0.4): hot-symbol aggregation, shared by the CLI (`ai`/`search`) and
/// the MCP `search` tool — both serialize `AiSearchOutput`, so folding here
/// covers both paths.
///
/// Symbols whose name matches ≥2 result blocks are folded into one
/// `AiAggregateGroup { symbol, file_count, match_count, top }` with the top-3
/// entries by score (duplicate code bodies deduped). Singles stay in the flat
/// `results` list. Runs after the engine's token-budget truncation and only
/// ever shrinks output, so budgets are never exceeded.
pub fn aggregate_results(out: &mut AiSearchOutput) {
    if out.results.len() < 2 {
        return;
    }

    // Count occurrences per symbol name (preserve encounter order).
    let mut order: Vec<String> = Vec::new();
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in &out.results {
        if r.name.is_empty() {
            continue; // unnamed blocks (file-level chunks) are never grouped
        }
        let c = counts.entry(r.name.clone()).or_insert(0);
        if *c == 0 {
            order.push(r.name.clone());
        }
        *c += 1;
    }

    let hot: Vec<&String> = order.iter().filter(|n| counts[*n] >= 2).collect();
    if hot.is_empty() {
        return;
    }

    let all = std::mem::take(&mut out.results);
    let mut flat: Vec<AiResultItem> = Vec::new();
    let mut grouped: std::collections::HashMap<String, Vec<AiResultItem>> =
        std::collections::HashMap::new();
    for r in all {
        if !r.name.is_empty() && counts.get(&r.name).copied().unwrap_or(0) >= 2 {
            grouped.entry(r.name.clone()).or_default().push(r);
        } else {
            flat.push(r);
        }
    }

    let mut groups: Vec<AiAggregateGroup> = Vec::new();
    for name in hot {
        let Some(mut items) = grouped.remove(name) else { continue };
        let match_count = items.len();
        let file_count = {
            let mut files: std::collections::HashSet<&str> = std::collections::HashSet::new();
            for it in &items {
                files.insert(it.path.as_str());
            }
            files.len()
        };
        // Highest score first, dedup identical code bodies, keep top-3.
        items.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        let mut seen_code: std::collections::HashSet<String> = std::collections::HashSet::new();
        items.retain(|it| seen_code.insert(format!("{}|{}", it.path, it.code)));
        items.truncate(3);
        groups.push(AiAggregateGroup {
            symbol: name.clone(),
            file_count,
            match_count,
            top: items,
        });
    }

    out.results = flat;
    out.aggregated = Some(groups);
}

fn find_matching_lines(code: &str, query: &str, base_line: usize) -> Vec<usize> {
    let query_terms: Vec<&str> = query
        .split_whitespace()
        .filter(|t| t.len() >= 2)
        .collect();
    if query_terms.is_empty() {
        return Vec::new();
    }
    code.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line_lower = line.to_lowercase();
            query_terms.iter().any(|term| line_lower.contains(&term.to_lowercase()))
        })
        .map(|(i, _)| base_line + i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, path: &str, score: f32, code: &str) -> AiResultItem {
        AiResultItem {
            score,
            path: path.to_string(),
            abs_path: format!("/abs/{path}"),
            lines: (1, 10),
            highlight_lines: vec![],
            kind: "function".to_string(),
            name: name.to_string(),
            signature: format!("fn {name}()"),
            summary: String::new(),
            code: code.to_string(),
            language: "rust".to_string(),
        }
    }

    fn output(results: Vec<AiResultItem>) -> AiSearchOutput {
        AiSearchOutput {
            query: "q".to_string(),
            mode: "expand",
            total_hits: results.len(),
            results,
            _ai_instructions: None,
            search_time_ms: 1,
            multi_hop: None,
            aggregated: None,
        }
    }

    #[test]
    fn hot_symbol_folds_to_top3_and_dedups() {
        let mut out = output(vec![
            item("foo", "a.rs", 0.9, "fn foo() { a }"),
            item("foo", "b.rs", 0.8, "fn foo() { b }"),
            item("foo", "c.rs", 0.7, "fn foo() { c }"),
            item("foo", "d.rs", 0.6, "fn foo() { d }"),
            item("foo", "d.rs", 0.5, "fn foo() { d }"), // duplicate body → deduped
            item("bar", "e.rs", 0.4, "fn bar() {}"),
        ]);
        let flat_tokens = serde_json::to_string(&out).unwrap().len();
        aggregate_results(&mut out);

        assert_eq!(out.results.len(), 1, "singles stay flat");
        assert_eq!(out.results[0].name, "bar");
        let groups = out.aggregated.as_ref().expect("aggregated present");
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.symbol, "foo");
        assert_eq!(g.file_count, 4);
        assert_eq!(g.match_count, 5);
        assert_eq!(g.top.len(), 3, "top-3 by score");
        assert!(g.top[0].score >= g.top[1].score);

        let folded_tokens = serde_json::to_string(&out).unwrap().len();
        assert!(folded_tokens < flat_tokens, "aggregation must shrink output");
    }

    #[test]
    fn no_duplicates_means_no_aggregation() {
        let mut out = output(vec![
            item("foo", "a.rs", 0.9, "fn foo() {}"),
            item("bar", "b.rs", 0.8, "fn bar() {}"),
        ]);
        aggregate_results(&mut out);
        assert_eq!(out.results.len(), 2);
        assert!(out.aggregated.is_none());
    }
}
