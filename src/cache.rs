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

const INDEX_VERSION: &str = "v2";

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
                return dir;
            }
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return canonical,
        }
    }
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

fn has_newer_files(dir: &Path, threshold: &std::time::SystemTime, depth: u32) -> bool {
    if depth > 8 {
        return false;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
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
