/*
 * globsearch/mod.rs
 * Project: sts-x 3.3.x
 * Description: Native `glob` subcommand + MCP tool — pure file-pattern
 *   discovery with AI-friendly ranking / truncation / zero-hit diagnosis.
 *
 * Replaces the system Glob calls (no ranking, no truncation, no diagnosis)
 * with the same ranked/truncated/diagnosed contract as `search`/`file`.
 *
 * Under the hood:
 *   - globset  : multi-pattern match (rg ecosystem, already a transitive dep
 *                of `ignore`), cross-platform consistent.
 *   - ignore   : gitignore-aware WalkBuilder (reuse the same walker pattern as
 *                filesearch, so `.git`/build products are skipped by default).
 *
 * Scoring (Phase 1 static heuristics): shallow paths > deep; source > test/
 * vendor; recent mtime boost (--sort-recent); git-modified boost (--git-aware);
 * hidden/dotfile penalty. Output is truncated by top_k AND max_tokens, and a
 * zero-hit run returns an extension-based diagnosis so the Agent can recover.
 */

use anyhow::Result;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Parsed + normalized request for a glob run.
pub struct GlobRequest {
    /// Raw pattern strings (comma-separated, `!`-prefixed = negation).
    pub patterns: Vec<String>,
    /// Search root directory.
    pub dir: PathBuf,
    /// Max matches to return (after ranking).
    pub top_k: usize,
    /// Token budget (0 = unlimited); ~ (chars/2) estimate per match.
    pub max_tokens: usize,
    /// Boost most-recently-modified files.
    pub sort_recent: bool,
    /// Boost git-modified / untracked files.
    pub git_aware: bool,
    /// Ignore .gitignore / hidden files when false; traverse everything when true.
    pub no_ignore: bool,
}

/// One matched file (plain, non-serialized — format.rs wraps it into AiGlobItem).
pub struct GlobHit {
    pub path: String,
    pub abs_path: String,
    pub size: u64,
    pub mtime: i64,
    pub score: f32,
    pub rank_signals: Vec<String>,
}

/// Result of a glob run.
pub struct GlobResult {
    pub hits: Vec<GlobHit>,
    pub total_hits: usize,
    /// Non-None only when total_hits == 0 (zero-hit self-rescue diagnosis).
    pub diagnosis: Option<String>,
}

/// Build positive and negation glob sets from a list of pattern strings.
fn build_glob_sets(patterns: &[String]) -> Result<(GlobSet, GlobSet)> {
    let mut pos = GlobSetBuilder::new();
    let mut neg = GlobSetBuilder::new();
    for raw in patterns {
        for p in raw.split(',') {
            let p = p.trim();
            if p.is_empty() {
                continue;
            }
            if let Some(rest) = p.strip_prefix('!') {
                neg.add(Glob::new(rest)?);
            } else {
                pos.add(Glob::new(p)?);
            }
        }
    }
    Ok((pos.build()?, neg.build()?))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Best-effort set of git-modified / untracked relative paths via
/// `git status --porcelain -z`. Empty on any failure (non-git dir, no git).
fn git_modified_set(dir: &Path) -> HashSet<String> {
    let out = std::process::Command::new("git")
        .args(["status", "--porcelain", "-z"])
        .current_dir(dir)
        .output();
    let mut set = HashSet::new();
    if let Ok(o) = out {
        if o.status.success() {
            let text = String::from_utf8_lossy(&o.stdout);
            for entry in text.split('\0') {
                if entry.len() < 4 {
                    continue;
                }
                // XY <path>  (renames: XY <old>\0<new>)
                let path = entry[3..].split('\0').next().unwrap_or("");
                if !path.is_empty() {
                    set.insert(path.to_string());
                }
            }
        }
    }
    set
}

/// Score a single hit. Returns (score, human-readable signals).
fn score(
    rel: &Path,
    depth: usize,
    mtime: i64,
    now: i64,
    git_mod: bool,
    sort_recent: bool,
) -> (f32, Vec<String>) {
    let mut score = 1.0_f32;
    let mut signals = Vec::new();

    // Depth: shallow paths rank higher.
    if depth <= 2 {
        score += 0.1;
        signals.push("shallow".to_string());
    } else {
        score -= 0.05 * (depth as f32 - 2.0).max(0.0);
    }

    // Test / vendor / build penalty.
    let rels = rel.to_string_lossy().to_lowercase();
    let penalties = [
        "test",
        "tests",
        "example",
        "examples",
        "target",
        "node_modules",
        "vendor",
        "dist",
        "build",
        "/.git/",
    ];
    if penalties.iter().any(|p| rels.contains(p)) {
        score -= 0.3;
        signals.push("non-source".to_string());
    } else {
        signals.push("source".to_string());
    }

    // Recent modification boost (only when --sort-recent).
    if sort_recent {
        let age = (now - mtime).max(0) as f32;
        // ~30-day half-life decay.
        let norm = (-age / (86_400.0 * 30.0)).exp();
        score += 0.2 * norm;
        signals.push("recent".to_string());
    }

    // Git-modified / untracked boost.
    if git_mod {
        score += 0.4;
        signals.push("git-modified".to_string());
    }

    // Dotfile penalty (walker usually skips these, but be safe).
    if rel
        .file_name()
        .map(|n| n.to_string_lossy().starts_with('.'))
        .unwrap_or(false)
    {
        score -= 0.5;
        signals.push("hidden".to_string());
    }

    (score, signals)
}

/// Rough token estimate for one match (JSON envelope + path + size).
fn estimate_tokens(rel: &Path, size: u64) -> usize {
    let path_tok = rel.to_string_lossy().chars().count().div_ceil(2);
    let size_tok = format!("{}", size).len().div_ceil(2);
    60 + path_tok + size_tok
}

/// Zero-hit self-rescue: scan the directory for the most common extensions
/// and suggest a broader pattern. Bilingual (CJK query → Chinese advice).
fn diagnose(dir: &Path, patterns: &[String], query_has_cjk: bool) -> String {
    use std::collections::HashMap;
    let mut exts: HashMap<String, usize> = HashMap::new();
    let walker = WalkBuilder::new(dir).standard_filters(true).build();
    let mut counted = 0usize;
    for entry in walker {
        if counted >= 4000 {
            break;
        }
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|f| f.is_file()).unwrap_or(false) {
            continue;
        }
        counted += 1;
        let ext = entry
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_lowercase()))
            .unwrap_or_else(|| "<none>".to_string());
        *exts.entry(ext).or_insert(0) += 1;
    }
    let mut top: Vec<(String, usize)> = exts.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    top.truncate(8);
    let ext_summary = top
        .iter()
        .map(|(e, c)| format!("{} ({})", e, c))
        .collect::<Vec<_>>()
        .join(", ");
    let pat = patterns.join(", ");
    if ext_summary.is_empty() {
        if query_has_cjk {
            format!("未匹配到「{}」。目录似乎为空或文件全被 .gitignore 忽略。可加 --no-ignore 包含被忽略的文件。", pat)
        } else {
            format!("No files matched '{}'. Directory appears empty or fully gitignored. Try --no-ignore to include ignored files.", pat)
        }
    } else if query_has_cjk {
        format!(
            "未匹配到「{}」。项目实际有：{}。可换更宽的模式（如 '**/*{}'）或加 --no-ignore。",
            pat,
            ext_summary,
            top.first().map(|(e, _)| e.as_str()).unwrap_or("*")
        )
    } else {
        format!("No files matched '{}'. Project has: {}. Try a broader pattern (e.g. '**/*{}') or --no-ignore.", pat, ext_summary, top.first().map(|(e, _)| e.as_str()).unwrap_or("*"))
    }
}

/// Run a glob search and return ranked, truncated, diagnosed results.
pub fn run_glob(req: &GlobRequest) -> Result<GlobResult> {
    let (pos, neg) = build_glob_sets(&req.patterns)?;
    let now = now_secs();

    let mut builder = WalkBuilder::new(&req.dir);
    if req.no_ignore {
        builder.standard_filters(false).hidden(false);
    } else {
        builder.standard_filters(true);
    }

    let git_set = if req.git_aware {
        git_modified_set(&req.dir)
    } else {
        HashSet::new()
    };

    struct Raw {
        rel: PathBuf,
        abs: PathBuf,
        depth: usize,
        size: u64,
        mtime: i64,
        git_mod: bool,
    }

    let mut raws: Vec<Raw> = Vec::new();
    for entry in builder.build() {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let ftype = match entry.file_type() {
            Some(ft) => ft,
            None => continue,
        };
        if !ftype.is_file() {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let rel = pathdiff::diff_paths(&abs, &req.dir).unwrap_or_else(|| abs.clone());
        let rel_s = rel.to_string_lossy();

        // Positive match required (try relative, then absolute for absolute patterns).
        if !pos.is_match(rel_s.as_ref()) && !pos.is_match(abs.to_string_lossy().as_ref()) {
            continue;
        }
        // Negation wins.
        if neg.is_match(rel_s.as_ref()) || neg.is_match(abs.to_string_lossy().as_ref()) {
            continue;
        }

        let meta = std::fs::metadata(&abs).ok();
        let (size, mtime) = match meta {
            Some(m) => {
                let size = m.len();
                let mtime = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                (size, mtime)
            }
            None => (0, 0),
        };
        let depth = rel.components().count().saturating_sub(1);
        let rel_str = rel_s.as_ref();
        let git_mod = git_set
            .iter()
            .any(|g| rel_str == g || rel_str.starts_with(&format!("{}/", g)));
        raws.push(Raw {
            rel,
            abs,
            depth,
            size,
            mtime,
            git_mod,
        });
    }

    let total_hits = raws.len();

    // Score + rank (highest score first, stable).
    let mut scored: Vec<(f32, Vec<String>, Raw)> = raws
        .into_iter()
        .map(|h| {
            let (s, sig) = score(&h.rel, h.depth, h.mtime, now, h.git_mod, req.sort_recent);
            (s, sig, h)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    // Truncate by top_k and max_tokens.
    let mut kept: Vec<(f32, Vec<String>, Raw)> = Vec::new();
    let mut tok_total = 0usize;
    for item in scored {
        if kept.len() >= req.top_k {
            break;
        }
        let est = estimate_tokens(&item.2.rel, item.2.size);
        if req.max_tokens > 0 && tok_total + est > req.max_tokens && !kept.is_empty() {
            break;
        }
        tok_total += est;
        kept.push(item);
    }

    let hits: Vec<GlobHit> = kept
        .into_iter()
        .map(|(score, sig, h)| GlobHit {
            path: h.rel.to_string_lossy().to_string(),
            abs_path: h.abs.display().to_string(),
            size: h.size,
            mtime: h.mtime,
            score,
            rank_signals: sig,
        })
        .collect();

    let query_has_cjk = req
        .patterns
        .iter()
        .any(|p| p.chars().any(|c| ('\u{3400}'..='\u{9fff}').contains(&c)));
    let diagnosis = if total_hits == 0 {
        Some(diagnose(&req.dir, &req.patterns, query_has_cjk))
    } else {
        None
    };

    Ok(GlobResult {
        hits,
        total_hits,
        diagnosis,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(patterns: &[&str], top_k: usize, max_tokens: usize) -> GlobRequest {
        GlobRequest {
            patterns: patterns.iter().map(|s| s.to_string()).collect(),
            dir: PathBuf::from("."),
            top_k,
            max_tokens,
            sort_recent: false,
            git_aware: false,
            no_ignore: false,
        }
    }

    #[test]
    fn glob_rs_only_matches_rs() {
        let r = run_glob(&req(&["**/*.rs"], 1000, 0)).unwrap();
        assert!(r.total_hits > 0, "should find .rs files in sts-x");
        for h in &r.hits {
            assert!(h.path.ends_with(".rs"), "unexpected match: {}", h.path);
        }
        assert!(r.diagnosis.is_none());
    }

    #[test]
    fn glob_top_k_truncates_and_omits() {
        let r = run_glob(&req(&["**/*"], 5, 0)).unwrap();
        assert!(r.hits.len() <= 5);
        let omitted = r.total_hits.saturating_sub(r.hits.len());
        assert!(omitted > 0, "expected some omitted, got {}", omitted);
    }

    #[test]
    fn glob_max_tokens_truncates() {
        // Tiny budget → only a handful of high-ranked files returned.
        let r = run_glob(&req(&["**/*"], 1000, 200)).unwrap();
        assert!(
            r.hits.len() < r.total_hits.min(1000),
            "budget should cut results"
        );
        assert!(r.total_hits.saturating_sub(r.hits.len()) > 0);
    }

    #[test]
    fn glob_zero_hit_diagnosis() {
        let r = run_glob(&req(&["**/*.zzznotext"], 20, 500)).unwrap();
        assert_eq!(r.total_hits, 0);
        assert!(r.diagnosis.is_some());
        let d = r.diagnosis.unwrap();
        assert!(
            d.contains("No files matched") || d.contains("未匹配"),
            "got: {d}"
        );
    }

    #[test]
    fn glob_negation_excludes_main() {
        let r = run_glob(&req(&["**/*.rs", "!**/main.rs"], 1000, 0)).unwrap();
        assert!(
            r.hits.iter().all(|h| h.path != "src/main.rs"),
            "main.rs should be excluded"
        );
    }

    #[test]
    fn glob_comma_separated_patterns() {
        // "*.rs,*.toml" as a single string (CLI passes one arg).
        let r = run_glob(&req(&["*.rs,*.toml"], 1000, 0)).unwrap();
        assert!(r.total_hits > 0);
        for h in &r.hits {
            let ok = h.path.ends_with(".rs") || h.path.ends_with(".toml");
            assert!(ok, "unexpected match: {}", h.path);
        }
    }
}
