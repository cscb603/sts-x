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
