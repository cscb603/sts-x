/*
 * filesearch/mod.rs
 * Project: sts-x 3.0
 * Description: Zero-index file search (filename + content) for ANY directory.
 *
 * Strategy:
 * - Prefers ripgrep (`rg`) when available on PATH — fast, gitignore-aware,
 *   works on unindexed dirs (e.g. ~/Downloads) with zero setup.
 * - Falls back to a gitignore-aware `ignore` walker when rg is absent
 *   (Windows without rg, minimal environments).
 * - Output is a flat list of FileMatch (path/size/mtime/matched_by/line/context),
 *   consumed by the `file` subcommand and the MCP `/file` endpoint.
 */

use crate::types::FileMatch;
use anyhow::Result;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Search files in `dir` for `query` (filename and/or content).
pub fn search_files(
    query: &str,
    dir: &Path,
    name_only: bool,
    top_k: usize,
    use_rg: bool,
) -> Result<Vec<FileMatch>> {
    if use_rg && rg_available() {
        match search_files_rg(query, dir, name_only, top_k) {
            Ok(m) => return Ok(m),
            Err(e) => {
                tracing::warn!("rg file search failed ({}), falling back to walker", e);
            }
        }
    }

    search_files_walk(query, dir, name_only, top_k)
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn search_files_rg(
    query: &str,
    dir: &Path,
    name_only: bool,
    top_k: usize,
) -> Result<Vec<FileMatch>> {
    let mut out: Vec<FileMatch> = Vec::new();

    // Content matches via rg --json (unambiguous cross-platform parsing)
    if !name_only {
        let out_cmd = Command::new("rg")
            .arg("-n")
            .arg("--no-heading")
            .arg("--with-filename")
            .arg("--line-number")
            .arg("--json")
            .arg("-I")
            .arg("-F")
            .arg("--")
            .arg(query)
            .arg(dir)
            .output()?;

        if out_cmd.status.success() {
            let text = String::from_utf8_lossy(&out_cmd.stdout);
            for line in text.lines() {
                if out.len() >= top_k {
                    break;
                }
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                    if v.get("type").and_then(|t| t.as_str()) != Some("match") {
                        continue;
                    }
                    let data = match v.get("data") {
                        Some(d) => d,
                        None => continue,
                    };
                    let path = data
                        .get("path")
                        .and_then(|p| p.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    let line_no = data
                        .get("line_number")
                        .and_then(|l| l.as_u64())
                        .unwrap_or(0) as usize;
                    let ctx = data
                        .get("lines")
                        .and_then(|l| l.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .trim_end_matches('\n')
                        .to_string();
                    out.push(build_match(&path, "content", line_no, ctx));
                }
            }
        }
    }

    // Filename matches via rg --files (respects gitignore)
    if out.len() < top_k {
        let files_cmd = Command::new("rg")
            .arg("--files")
            .arg("--no-messages")
            .arg(dir)
            .output()?;
        if files_cmd.status.success() {
            let text = String::from_utf8_lossy(&files_cmd.stdout);
            let ql = query.to_lowercase();
            for line in text.lines() {
                if out.len() >= top_k {
                    break;
                }
                if line.to_lowercase().contains(&ql) {
                    out.push(build_match(line, "name", 0, String::new()));
                }
            }
        }
    }

    Ok(out)
}

fn search_files_walk(
    query: &str,
    dir: &Path,
    name_only: bool,
    top_k: usize,
) -> Result<Vec<FileMatch>> {
    let mut out: Vec<FileMatch> = Vec::new();
    let ql = query.to_lowercase();

    let walker = ignore::WalkBuilder::new(dir)
        .git_ignore(true)
        .parents(true)
        .standard_filters(true)
        .build();

    for entry in walker {
        if out.len() >= top_k {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
            continue;
        }
        let path = entry.path();
        let rel = pathdiff::diff_paths(path, dir).unwrap_or_else(|| path.to_path_buf());
        let rel_str = rel.display().to_string();

        // filename match
        if rel_str.to_lowercase().contains(&ql) {
            out.push(build_match(&rel_str, "name", 0, String::new()));
            continue;
        }

        // content match (non-code/text only — skip binaries by read_to_string)
        if !name_only {
            if let Ok(content) = std::fs::read_to_string(path) {
                if let Some(idx) = content
                    .lines()
                    .position(|l| l.to_lowercase().contains(&ql))
                {
                    out.push(build_match(
                        &rel_str,
                        "content",
                        idx + 1,
                        content.lines().nth(idx).unwrap_or("").to_string(),
                    ));
                }
            }
        }
    }

    Ok(out)
}

fn build_match(path: &str, matched_by: &str, line: usize, context: String) -> FileMatch {
    let abs = if path.starts_with('/')
        || (path.len() >= 2 && path.as_bytes()[1] == b':')
    {
        PathBuf::from(path)
    } else {
        PathBuf::from(path)
    };
    let meta = std::fs::metadata(&abs);
    let (size, mtime) = match meta {
        Ok(m) => {
            let size = m.len();
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            (size, mtime)
        }
        Err(_) => (0, 0),
    };
    FileMatch {
        path: path.to_string(),
        abs_path: abs.display().to_string(),
        size,
        mtime,
        is_dir: false,
        matched_by: matched_by.to_string(),
        line,
        context,
    }
}
