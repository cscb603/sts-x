pub mod format;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single code block extracted by AST chunking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeBlock {
    /// Relative path from project root
    pub path: PathBuf,
    /// Absolute path on disk
    pub abs_path: PathBuf,
    /// Type: function, class, method, module, block
    pub kind: BlockKind,
    /// Symbol name (function name, class name, etc.)
    pub name: String,
    /// Function/class signature line
    pub signature: String,
    /// Doc comment or inline summary
    pub doc_comment: String,
    /// Full source code of this block
    pub code: String,
    /// Programming language
    pub language: String,
    /// Line range in source file
    pub start_line: usize,
    pub end_line: usize,
    /// Import statements found near this block
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

/// Search mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Search code blocks (AST chunks, default)
    Code,
    /// Search file names only
    Filename,
    /// Search all files content (code + non-code)
    All,
}

impl Default for SearchMode {
    fn default() -> Self {
        Self::Code
    }
}

/// Search query
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    /// Natural language query
    pub query: String,
    /// Search mode
    #[serde(default)]
    pub mode: SearchMode,
    /// Optional language filter
    #[serde(default)]
    pub languages: Option<Vec<String>>,
    /// Optional path filter (glob)
    #[serde(default)]
    pub path_filter: Option<String>,
    /// Max results
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    /// Enable multi-hop decomposition
    #[serde(default)]
    pub multi_hop: bool,
}

fn default_top_k() -> usize { 5 }

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            query: String::new(),
            mode: SearchMode::default(),
            languages: None,
            path_filter: None,
            top_k: 5,
            multi_hop: false,
        }
    }
}

/// A single search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Relevance score (0.0 - 1.0)
    pub score: f32,
    /// Matching code block
    pub block: CodeBlock,
    /// Highlighted code lines (line numbers that matched)
    pub highlight_lines: Vec<usize>,
    /// Match explanation (from reranker or LLM)
    pub explanation: String,
}

/// Full search response — AI-optimized format
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    /// Original query
    pub query: String,
    /// Search results in ranked order
    pub results: Vec<SearchResult>,
    /// Total number of hits
    pub total_hits: usize,
    /// Search time in milliseconds
    pub search_time_ms: u64,
    /// Multi-hop sub-queries and their results (if enabled)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub multi_hop: Option<Vec<MultiHopStep>>,
}

/// Multi-hop decomposition step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiHopStep {
    pub sub_query: String,
    pub results: Vec<SearchResult>,
    pub search_time_ms: u64,
}

/// Index configuration
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
            index_path: PathBuf::from(".stsx-index"),
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
