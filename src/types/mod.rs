pub mod format;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    pub path: PathBuf,
    pub abs_path: PathBuf,
    pub kind: BlockKind,
    pub name: String,
    pub signature: String,
    pub doc_comment: String,
    pub code: String,
    pub language: String,
    pub start_line: usize,
    pub end_line: usize,
    pub imports: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BlockKind {
    Function,
    Class,
    Method,
    Module,
    Block,
    Struct,
    Enum,
    Trait,
    Impl,
    Interface,
    Type,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Code,
    Filename,
    All,
}

/// Output verbosity mode (STS-X 3.0 progressive disclosure).
/// - `Expand` (default): return the full AST block (function/class/method) so the
///   AI can read/modify it. Token-heavy but complete.
/// - `Locate`: return only the matching line(s) + small context, grep-sized (~130 tok).
///   Used for "first locate", then optionally `--expand` a specific symbol.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
    Expand,
    Locate,
}

/// A single located match line (used by `--locate` output mode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocateMatch {
    pub score: f32,
    pub file: String,
    pub abs_path: String,
    pub line: usize,
    pub context: String,
    pub kind: String,
    pub name: String,
}

/// A file-system match (used by the `file` subcommand).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMatch {
    pub path: String,
    pub abs_path: String,
    pub size: u64,
    pub mtime: i64,
    pub is_dir: bool,
    /// "name" | "content" — how this entry matched.
    pub matched_by: String,
    /// line number when matched by content (0 for name-only)
    pub line: usize,
    /// the matching line content (empty for name-only)
    pub context: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    #[serde(default)]
    pub mode: SearchMode,
    /// 3.0: progressive-disclosure output. Expand=full block, Locate=line-level.
    #[serde(default)]
    pub output_mode: OutputMode,
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    #[serde(default)]
    pub path_filter: Option<String>,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default = "default_context")]
    pub context_lines: usize,
    #[serde(default)]
    pub multi_hop: bool,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub filename: bool,
    #[serde(default)]
    pub all: bool,
}

fn default_top_k() -> usize { 2 }
fn default_context() -> usize { 0 }

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::default(),
            output_mode: OutputMode::default(),
            languages: None,
            path_filter: None,
            top_k: 2,
            // 0 = full block (expand default is complete AST block, not a window)
            context_lines: 0,
            multi_hop: false,
            path: None,
            filename: false,
            all: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub score: f32,
    pub block: CodeBlock,
    pub highlight_lines: Vec<usize>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub results: Vec<SearchResult>,
    pub total_hits: usize,
    pub search_time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_hop: Option<Vec<MultiHopStep>>,
    /// 3.0 locate-mode matches (grep-sized line hits). Empty unless output_mode==Locate.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub locate_matches: Vec<LocateMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiHopStep {
    pub sub_query: String,
    pub results: Vec<SearchResult>,
    pub search_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexConfig {
    pub project_root: PathBuf,
    pub index_path: PathBuf,
    pub model_path: PathBuf,
    pub languages: Vec<String>,
    pub exclude_patterns: Vec<String>,
    pub embedding_dim: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            project_root: PathBuf::from("."),
            index_path: PathBuf::from(""),
            model_path: PathBuf::from("models"),
            languages: vec![
                "rust".into(),
                "python".into(),
                "javascript".into(),
                "typescript".into(),
                "go".into(),
                "java".into(),
                "php".into(),
                "ruby".into(),
                "swift".into(),
                "scala".into(),
            ],
            exclude_patterns: vec![
                "node_modules/*".into(),
                "target/*".into(),
                ".git/*".into(),
                "vendor/*".into(),
                "dist/*".into(),
                "build/*".into(),
                ".venv/*".into(),
                "__pycache__/*".into(),
            ],
            embedding_dim: 384,
        }
    }
}
