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

const PROJECT_MARKERS: &[&str] = &[
    ".git", "Cargo.toml", "package.json", "go.mod", "pyproject.toml",
    "setup.py", "Makefile", "CMakeLists.txt", "build.gradle", "pom.xml",
    ".stsx-root",
];

pub fn detect_project_root(start: &Path) -> PathBuf {
    let canonical = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut dir = if canonical.is_file() {
        canonical.parent().unwrap_or(&canonical).to_path_buf()
    } else {
        canonical.clone()
    };
    loop {
        for marker in PROJECT_MARKERS {
            if dir.join(marker).exists() {
                // R4: a Cargo workspace member escalates to the workspace root
                // (guardrailed: depth≤8, stops at .stsx-root, never $HOME or /).
                return escalate_to_workspace_root(dir);
            }
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
    use super::INDEX_VERSION;

    #[test]
    fn index_version_is_v3_for_cjk_rebuild() {
        // v2 indexes used SimpleTokenizer (CJK = 1 giant token) and must be
        // invalidated by the v3 bump; this test pins the constant.
        assert_eq!(INDEX_VERSION, "v3");
    }
}
