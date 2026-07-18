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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Code,
    Filename,
    All,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    pub query: String,
    #[serde(default)]
    pub mode: SearchMode,
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

fn default_top_k() -> usize { 3 }
fn default_context() -> usize { 5 }

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::default(),
            languages: None,
            path_filter: None,
            top_k: 3,
            context_lines: 5,
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
