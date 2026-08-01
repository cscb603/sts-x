/*
 * cache.rs
 * Project: sts-x
 * Description: Cross-platform cache directory management + project root detection
 *
 * Indexes go to system cache, never polluting project directories:
 * - macOS: ~/Library/Caches/sts-x/<hash>/
 * - Linux: ~/.cache/sts-x/<hash>/
 * - Windows: %LOCALAPPDATA%\sts-x\cache\<hash>\
 */

use std::path::{Path, PathBuf};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

fn path_hash(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut hasher = DefaultHasher::new();
    canonical.hash(&mut hasher);
    let h = hasher.finish();
    format!("{:016x}", h)
}

pub fn cache_root() -> PathBuf {
    // 优先使用基座库 core_lib 的跨平台缓存目录实现（与星TAP全家桶保持一致）。
    // 失败时降级到本地 dirs 实现，保证单二进制分发的健壮性。
    match core_lib::path::cache_dir("sts-x") {
        Ok(dir) => dir,
        Err(_) => fallback_cache_root(),
    }
}

// 本地降级实现：仅在基座库不可用时启用，保持与旧行为一致。
fn fallback_cache_root() -> PathBuf {
    let base = if cfg!(target_os = "macos") {
        dirs::cache_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Library/Caches")
        })
    } else if cfg!(target_os = "windows") {
        dirs::cache_dir().unwrap_or_else(|| {
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    dirs::home_dir()
                        .unwrap_or_else(|| PathBuf::from("."))
                        .join("AppData/Local")
                })
        })
    } else {
        dirs::cache_dir().unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".cache")
        })
    };
    base.join("sts-x")
}

// v3: CJK bigram tokenizer introduced (index terms changed) → old v2 indexes must rebuild.
const INDEX_VERSION: &str = "v3";

pub fn index_dir_for(project_root: &Path) -> PathBuf {
    let hash = path_hash(project_root);
    cache_root().join(INDEX_VERSION).join(hash)
}

pub fn resolve_index_path(project_root: &Path, custom: Option<&PathBuf>) -> PathBuf {
    if let Some(c) = custom {
        c.clone()
    } else {
        index_dir_for(project_root)
    }
}

/// Strong project markers — alone they prove a real project root.
/// (`.stsx-root` is the explicit user pin and must stay strongest.)
const STRONG_MARKERS: &[&str] = &[
    ".git", "Cargo.toml", "go.mod", "pyproject.toml", "setup.py",
    "Makefile", "CMakeLists.txt", "build.gradle", "pom.xml", ".stsx-root",
];

/// Weak markers — only count as a project root when corroborated.
/// `package.json` alone is NOT enough: aggregate/monorepo ROOT dirs commonly
/// carry a lone `package.json` (e.g. a puppeteer/playwright config) with no
/// real node project. Require `node_modules/` next to it (a real npm project
/// has deps installed locally). This kills the F:\trae-cn style false root
/// without breaking genuine Node projects.
fn is_project_root(dir: &Path) -> bool {
    for marker in STRONG_MARKERS {
        if dir.join(marker).exists() {
            return true;
        }
    }
    if dir.join("package.json").exists() && dir.join("node_modules").is_dir() {
        return true;
    }
    false
}

/// Cap on upward climb inside `detect_project_root` (same spirit as the
/// depth≤8 guardrail in `escalate_to_workspace_root`). Without it, a search
/// deep inside a non-project subtree could climb all the way to a drive root.
const MAX_ROOT_CLIMB: usize = 8;

pub fn detect_project_root(start: &Path) -> PathBuf {
    let canonical = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir = if canonical.is_file() {
        canonical.parent().unwrap_or(&canonical).to_path_buf()
    } else {
        canonical.clone()
    };
    let mut climbed = 0;
    loop {
        if is_project_root(&dir) {
            // R4: a Cargo workspace member escalates to the workspace root
            // (guardrailed: depth≤8, stops at .stsx-root, never $HOME or /).
            return escalate_to_workspace_root(dir);
        }
        climbed += 1;
        if climbed >= MAX_ROOT_CLIMB {
            return canonical;
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return canonical,
        }
    }
}

/// R4 (v0.4): if `found` is a Cargo workspace member, climb to the real
/// workspace root so searches inside one member cover all members.
///
/// BM25 guardrails (whitepaper §7.4 — this must NEVER become a global search):
/// - climbs at most 8 levels (same cap as `has_newer_files`);
/// - `.stsx-root` marker stops the climb immediately (explicit user pin);
/// - never escalates to `$HOME` or the filesystem root;
/// - only escalates when an ancestor `Cargo.toml` actually declares
///   `[workspace]` — otherwise returns `found` unchanged (project-level).
fn escalate_to_workspace_root(found: PathBuf) -> PathBuf {
    // Explicit pin or non-Cargo project → no escalation.
    if found.join(".stsx-root").exists() || !found.join("Cargo.toml").exists() {
        return found;
    }
    // Already a workspace root (own Cargo.toml declares [workspace]).
    if cargo_toml_declares_workspace(&found.join("Cargo.toml")) {
        return found;
    }

    let home = dirs::home_dir();
    let mut dir = found.clone();
    for _ in 0..8 {
        let parent = match dir.parent() {
            Some(p) if p != dir => p.to_path_buf(),
            _ => break,
        };
        // Hard stop: never treat $HOME or / as a workspace root.
        if home.as_deref() == Some(parent.as_path()) || parent.parent().is_none() {
            break;
        }
        // .stsx-root on an ancestor pins that ancestor as the root.
        if parent.join(".stsx-root").exists() {
            return parent;
        }
        let manifest = parent.join("Cargo.toml");
        if manifest.exists() && cargo_toml_declares_workspace(&manifest) {
            return parent;
        }
        dir = parent;
    }
    found
}

/// True if the Cargo.toml at `path` declares a `[workspace]` table
/// (also matches `[workspace.members]` / `[workspace.dependencies]` headers).
fn cargo_toml_declares_workspace(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.lines().any(|l| l.trim_start().starts_with("[workspace")))
        .unwrap_or(false)
}

const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", "__pycache__",
    ".venv", "venv", ".tox", ".mypy_cache", ".pytest_cache",
    "vendor", ".next", ".nuxt", ".output",
];

pub fn is_index_stale(index_path: &Path, project_root: &Path) -> bool {
    let tantivy_dir = index_path.join("tantivy");
    let meta_path = tantivy_dir.join("meta.json");
    if !meta_path.exists() {
        return true;
    }

    let index_mtime = match meta_path.metadata().and_then(|m| m.modified()) {
        Ok(t) => t,
        Err(_) => return true,
    };

    has_newer_files(project_root, &index_mtime, 0)
}

/// P2-1: cap the stale-scan tree walk — very deep / huge trees (monorepos,
/// vendored docs) must not turn a first search into a multi-second walk.
const MAX_STALE_DEPTH: u32 = 6;
const MAX_STALE_FILES: usize = 5_000;

fn has_newer_files(dir: &Path, threshold: &std::time::SystemTime, depth: u32) -> bool {
    if depth > MAX_STALE_DEPTH {
        return false;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    let mut scanned = 0usize;
    for entry in entries.flatten() {
        scanned += 1;
        if scanned > MAX_STALE_FILES {
            // Too many entries to scan cheaply — force a rebuild instead of
            // risking a stale index on a huge tree (rebuild is bounded work).
            return true;
        }
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with('.') && name != ".github" && name != ".config" {
            continue;
        }
        if SKIP_DIRS.contains(&name) {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if mtime > *threshold {
                    return true;
                }
            }
            if meta.is_dir() && has_newer_files(&path, threshold, depth + 1) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{detect_project_root, is_project_root, INDEX_VERSION};
    use std::fs;

    #[test]
    fn index_version_is_v3_for_cjk_rebuild() {
        // v2 indexes used SimpleTokenizer (CJK = 1 giant token) and must be
        // invalidated by the v3 bump; this test pins the constant.
        assert_eq!(INDEX_VERSION, "v3");
    }

    /// Aggregate-root regression (F:\trae-cn style): a lone `package.json`
    /// with NO node_modules must NOT make the directory look like a project
    /// root — otherwise every child-project search climbs to the aggregate
    /// root and indexes 380k blocks.
    #[test]
    fn lone_package_json_is_not_a_project_root() {
        // Unique base per test: cargo runs tests in parallel and shared
        // `stsx-root-test-<pid>` dirs race — one test's remove_dir_all can
        // delete another test's fixtures mid-run (macOS: canonicalize then
        // fails → unwrap_or falls back to the un-resolved /var path).
        let base = std::env::temp_dir().join(format!("stsx-root-test-{}-lone", std::process::id()));
        let agg = base.join("aggregate"); // aggregate dir with lone package.json
        fs::create_dir_all(&agg).ok();
        fs::write(agg.join("package.json"), r#"{"dependencies":{"puppeteer":"^24"}}"#).ok();
        assert!(!is_project_root(&agg), "lone package.json must not be a root");
        fs::remove_dir_all(&base).ok();
    }

    /// Real Node project: package.json + node_modules → project root.
    #[test]
    fn package_json_with_node_modules_is_a_project_root() {
        let base = std::env::temp_dir().join(format!("stsx-root-test-{}-node", std::process::id()));
        let proj = base.join("real-node-project");
        fs::create_dir_all(proj.join("node_modules")).ok();
        fs::write(proj.join("package.json"), "{}").ok();
        assert!(is_project_root(&proj), "package.json + node_modules must be a root");
        fs::remove_dir_all(&base).ok();
    }

    /// Child project under an aggregate dir with lone package.json: detection
    /// must land on the CHILD, not climb to the aggregate root.
    #[test]
    fn child_project_does_not_climb_to_lone_package_json_aggregate() {
        let base = std::env::temp_dir().join(format!("stsx-root-test-{}-child", std::process::id()));
        let agg = base.join("aggregate");
        let child = agg.join("my-lib");
        fs::create_dir_all(&child).ok();
        fs::write(agg.join("package.json"), "{}").ok(); // lone, no node_modules
        fs::write(child.join("Cargo.toml"), "[package]\nname=\"my-lib\"\n").ok(); // strong marker
        let detected = detect_project_root(&child);
        // canonicalize() may add the \\?\ long-path prefix on Windows; compare canonical forms.
        let want = child.canonicalize().unwrap_or(child.clone());
        assert_eq!(detected, want, "should land on child (Cargo.toml), got {detected:?}");
        fs::remove_dir_all(&base).ok();
    }

    /// Depth cap: deep non-project subtree must NOT climb to a drive root —
    /// it should stop after MAX_ROOT_CLIMB and return the original path.
    #[test]
    fn deep_subtree_without_markers_returns_original() {
        let base = std::env::temp_dir().join(format!("stsx-root-test-{}-deep", std::process::id()));
        let deep = base
            .join("a")
            .join("b")
            .join("c")
            .join("d")
            .join("e")
            .join("f")
            .join("g")
            .join("h")
            .join("i")
            .join("j");
        fs::create_dir_all(&deep).ok();
        let detected = detect_project_root(&deep);
        let want = deep.canonicalize().unwrap_or(deep.clone());
        assert_eq!(
            detected,
            want,
            "deep markerless subtree should return original, got {detected:?}"
        );
        fs::remove_dir_all(&base).ok();
    }
}
