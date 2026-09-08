/*
 * guard.rs — 索引单元护栏 + 项目可发现性（v3.3.3+ Mac 端重新设计）
 *
 * # 为什么要这个模块
 *
 * sts-x 原本把「传给它的任意目录」当成一个索引单元（key = 路径 hash）。
 * 这在真实使用中会踩到一个结构性问题：
 *
 *   AI 工作区根（如 /Users/xtap/Documents/AI）是**多个独立项目的容器**
 *   （本机 291 个子项目，其中 30 个一级子目录各自带项目标记），
 *   但它本身既不是单一项目、也不该被当成一个项目来索引。
 *   → 实测产生 80,107 blocks / 110MB 的巨型索引，且工作区文件频繁变动
 *     导致索引常年 STALE，每次 AI 不传 path 的调用都触发分钟级重建。
 *
 * # 行业惯例（设计依据）
 *
 * - **Zoekt / Sourcegraph**：索引单元 = 仓库（repository），一个仓库一个 shard；
 *   从不为「包含多个仓库的父目录」建立索引单元；大仓库拆多个 shard。
 * - **Cursor**：理想索引文件数 500–2000，超 5000 明显变慢；monorepo 明确建议
 *   「只索引当前工作的子包，不要打开 monorepo 根」。
 * - **Claude Code**：干脆不预索引，按需 grep/glob，靠 AGENTS.md 提供项目地图。
 *
 * 共同点：**索引单元是「项目/仓库」，不是「目录」**。
 *
 * # 本模块的策略
 *
 * 1. **判定目标类型**（见 `classify`）：单项目 / monorepo → 正常索引；
 *    多项目容器 → 不建索引。
 * 2. **容器不静默失败**：降级为跨子项目的零索引 file 搜索（rg 后端，秒级），
 *    并附带 `candidate_projects`，让 AI 知道下一步该精确搜哪个项目。
 *    —— 这同时解决了「AI 不知道该传什么 path」的可发现性问题。
 * 3. **逃生舱**：`.stsx-root` 标记或 `STX_ALLOW_MULTI_ROOT=1` 强制索引。
 *
 * # 注意：不要只看 .git
 *
 * 容器目录自身也可能有 .git（本机 AI 工作区就有），所以判定依据是
 * 「一级子目录中有多少个**独立项目**」，而不是「自己有没有 .git」。
 * 反过来，monorepo 内部虽有 packages/<pkg>/package.json，但它们都在同一个
 * `packages/` 聚合目录下，一级子目录自身无标记 → 正确判定为单项目。
 */

use crate::filesearch;
use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};

/// 判定为「容器」所需的最少一级子项目数。
///
/// 取 4：单体仓库内部可能有一两个带标记的子目录，但不会同时有 4 个平级的
/// 独立项目。本机工作区有 30 个，远超阈值。
const CONTAINER_MIN_PROJECTS: usize = 4;

/// 一级子目录中的项目标记（命中任意一个即计为一个独立项目）。
///
/// 只看**直接子目录**（depth 1），不递归 —— 成本 O(顶层条目数)，
/// 且不会误伤 `crates/`、`packages/` 这类聚合目录（它们自身没有标记）。
const PROJECT_MARKERS: [&str; 8] = [
    "Cargo.toml",
    "package.json",
    ".git",
    "pyproject.toml",
    "go.mod",
    "pom.xml",
    "build.gradle",
    "CMakeLists.txt",
];

/// 目标目录的索引单元类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// 单一项目 / monorepo —— 正常建立 AST + BM25 索引。
    Project,
    /// 多个独立项目的容器（工作区根 / 聚合目录）—— 不建索引。
    Container,
}

/// 一个被发现的子项目（用于可发现性）。
#[derive(Debug, Clone)]
pub struct FoundProject {
    pub name: String,
    pub path: PathBuf,
    /// 命中了哪个标记（Cargo.toml / .git / ...）
    pub marker: String,
}

/// 列出 `root` 一级子目录中的独立项目（按名称排序，最多 `limit` 个）。
pub fn list_projects(root: &Path, limit: usize) -> Vec<FoundProject> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<FoundProject> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let marker = PROJECT_MARKERS
            .iter()
            .find(|m| path.join(m).exists())
            .map(|m| m.to_string());
        if let Some(marker) = marker {
            out.push(FoundProject {
                name: entry.file_name().to_string_lossy().into_owned(),
                path,
                marker,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(limit);
    out
}

/// 判定目标目录属于哪种索引单元。
pub fn classify(root: &Path) -> TargetKind {
    // 逃生舱 1：环境变量显式放行
    if std::env::var("STX_ALLOW_MULTI_ROOT")
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
    {
        return TargetKind::Project;
    }
    // 逃生舱 2：用户在该目录放 .stsx-root 显式钉住「这就是一个项目」
    if root.join(".stsx-root").exists() {
        return TargetKind::Project;
    }

    // 一级子目录中的独立项目数（limit 用小阈值即可，够判定就早停）
    let n = count_child_projects(root, CONTAINER_MIN_PROJECTS);
    if n >= CONTAINER_MIN_PROJECTS {
        tracing::info!(
            "guard: {} is a multi-project container ({} child projects) — skipping index",
            root.display(),
            n
        );
        TargetKind::Container
    } else {
        TargetKind::Project
    }
}

/// 便捷判定：是否为容器（不该建索引）。
pub fn is_container(root: &Path) -> bool {
    matches!(classify(root), TargetKind::Container)
}

fn count_child_projects(root: &Path, early_stop_at: usize) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut n = 0usize;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if PROJECT_MARKERS.iter().any(|m| path.join(m).exists()) {
            n += 1;
            if n >= early_stop_at {
                return n;
            }
        }
    }
    n
}

/// 容器目录上的降级搜索：零索引、rg 后端、跨全部子项目，秒级返回。
///
/// 响应里带：
/// - `degraded: true` + `reason` —— 不静默，AI 知道这是降级路径
/// - `candidate_projects` —— 可发现性：AI 可以直接挑一个项目精确搜
/// - `matched_projects` —— 本次命中落在哪些项目里（帮 AI 缩小范围）
pub fn degraded_search(
    query: &str,
    root: &Path,
    top_k: usize,
    name_only: bool,
) -> Result<serde_json::Value> {
    let start = std::time::Instant::now();
    let matches = filesearch::search_files(query, root, name_only, top_k, true)?;
    let elapsed = start.elapsed().as_millis() as u64;

    // 命中结果落在哪些子项目里 → 帮 AI 缩小范围
    let mut matched: Vec<String> = Vec::new();
    for m in &matches {
        // rg 后端可能只给相对路径：优先 abs_path，回落到 path
        let raw = if m.abs_path.is_empty() {
            &m.path
        } else {
            &m.abs_path
        };
        let p = Path::new(raw);
        // 先尝试剥掉 root 前缀（绝对路径），失败则按相对路径取首段
        let first = match p.strip_prefix(root) {
            Ok(rel) => rel.components().next(),
            Err(_) => p.components().next(),
        };
        if let Some(comp) = first {
            let name = comp.as_os_str().to_string_lossy().into_owned();
            if !name.is_empty() && name != "/" && !matched.contains(&name) {
                matched.push(name);
            }
        }
    }

    let candidates: Vec<serde_json::Value> = list_projects(root, 20)
        .into_iter()
        .map(
            |p| json!({ "name": p.name, "path": p.path.display().to_string(), "marker": p.marker }),
        )
        .collect();

    Ok(json!({
        "query": query,
        "mode": "degraded-file",
        "degraded": true,
        "reason": format!(
            "`{}` 是包含多个独立项目的容器目录，sts-x 不会为它建立 AST/BM25 索引（索引单元 = 项目/仓库，不是目录）",
            root.display()
        ),
        "hint": "当前结果是跨全部子项目的零索引文件内容搜索（rg 后端），可直接用。要启用 AST/BM25 精确搜索，请把 path 指向下面 candidate_projects 里的某个项目。",
        "searched_root": root.display().to_string(),
        "matched_projects": matched,
        "candidate_projects": candidates,
        "total_hits": matches.len(),
        "elapsed_ms": elapsed,
        "matches": matches,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_container(tmp: &Path, n: usize) {
        for i in 0..n {
            let p = tmp.join(format!("proj{i}"));
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("Cargo.toml"), "[package]\n").unwrap();
        }
    }

    #[test]
    fn container_detected_with_many_child_projects() {
        let tmp = std::env::temp_dir().join("stsx_guard_container_test");
        let _ = fs::remove_dir_all(&tmp);
        make_container(&tmp, 5);
        assert_eq!(classify(&tmp), TargetKind::Container);
        assert_eq!(list_projects(&tmp, 20).len(), 5);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn monorepo_is_still_a_project() {
        // packages/<pkg>/package.json 都在聚合目录下，一级子目录自身无标记
        let tmp = std::env::temp_dir().join("stsx_guard_mono_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("packages").join("a")).unwrap();
        fs::create_dir_all(tmp.join("packages").join("b")).unwrap();
        fs::write(tmp.join("packages").join("a").join("package.json"), "{}").unwrap();
        fs::write(tmp.join("packages").join("b").join("package.json"), "{}").unwrap();
        fs::write(tmp.join("Cargo.toml"), "[package]\n").unwrap();
        assert_eq!(classify(&tmp), TargetKind::Project); // 不该被误伤
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn cargo_workspace_is_still_a_project() {
        // crates/<name>/Cargo.toml —— 一级子目录 crates/ 自身无标记
        let tmp = std::env::temp_dir().join("stsx_guard_ws_test");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("crates").join("a")).unwrap();
        fs::write(
            tmp.join("crates").join("a").join("Cargo.toml"),
            "[package]\n",
        )
        .unwrap();
        fs::write(tmp.join("Cargo.toml"), "[workspace]\n").unwrap();
        assert_eq!(classify(&tmp), TargetKind::Project);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn stsx_root_marker_pins_the_directory() {
        let tmp = std::env::temp_dir().join("stsx_guard_pin_test");
        let _ = fs::remove_dir_all(&tmp);
        make_container(&tmp, 5);
        fs::write(tmp.join(".stsx-root"), "").unwrap();
        assert_eq!(classify(&tmp), TargetKind::Project); // 显式钉住 → 放行
        let _ = fs::remove_dir_all(&tmp);
    }
}
