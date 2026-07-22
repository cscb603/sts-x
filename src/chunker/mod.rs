/*
 * chunker/mod.rs
 * Project: sts-x
 * Description: tree-sitter AST-based code chunking engine
 *
 * Core responsibility: Parse source files into semantic blocks
 * (functions, classes, methods, structs, enums, traits, etc.)
 * using tree-sitter AST, with gitignore-aware file walking.
 */

use crate::types::{BlockKind, CodeBlock};
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::path::Path;
use tree_sitter::{Parser, Language};

/// Language configuration mapping
fn get_language(lang: &str) -> Option<Language> {
    match lang {
        "rust" => Some(tree_sitter_rust::language()),
        "python" => Some(tree_sitter_python::language()),
        "javascript" => Some(tree_sitter_javascript::language()),
        "typescript" => Some(tree_sitter_typescript::language_typescript()),
        "tsx" => Some(tree_sitter_typescript::language_tsx()),
        "java" => Some(tree_sitter_java::language()),
        "c" => Some(tree_sitter_c::language()),
        "cpp" => Some(tree_sitter_cpp::language()),
        "go" => Some(tree_sitter_go::language()),
        _ => None,
    }
}

fn extension_to_language(ext: &str) -> Option<&'static str> {
    match ext {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "jsx" | "mjs" => Some("javascript"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "go" => Some("go"),
        "java" => Some("java"),
        "c" => Some("c"),
        "cpp" | "cc" | "cxx" | "hpp" => Some("cpp"),
        _ => None,
    }
}

/// Code chunker using tree-sitter AST
pub struct Chunker {
    parsers: Vec<(String, Parser)>,
}

impl Chunker {
    /// Create a new Chunker with tree-sitter parsers for configured languages
    pub fn new(languages: &[String]) -> Result<Self> {
        let mut parsers = Vec::new();
        for lang in languages {
            if let Some(language) = get_language(lang) {
                let mut parser = Parser::new();
                parser
                    .set_language(&language)
                    .with_context(|| format!("Failed to set tree-sitter parser for language: {}", lang))?;
                parsers.push((lang.clone(), parser));
            } else {
                tracing::warn!("Unsupported language: {}, skipping", lang);
            }
        }
        Ok(Self { parsers })
    }

    /// Walk a project directory and extract all code blocks
    pub fn index_project(&mut self, root: &Path, config: &crate::types::IndexConfig) -> Result<Vec<CodeBlock>> {
        let mut all_blocks = Vec::new();

        let walker = WalkBuilder::new(root)
            .git_ignore(true)
            .parents(true)
            .standard_filters(true)
            .build();

        for entry in walker {
            let entry = entry?;
            if !entry.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                continue;
            }

            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

            // Check if this extension is excluded
            let rel_path = path.strip_prefix(root).unwrap_or(path);
            let rel_str = rel_path.display().to_string();
            if config.exclude_patterns.iter().any(|p| {
                let pattern = p.trim_end_matches("/*");
                rel_str.starts_with(pattern) || rel_str.contains("/target/")
            }) {
                continue;
            }

            // Skip noise/backup paths
            let noise_patterns = ["_backup", "_original", "_old", "_copy", "复制", "副本", ".bak", ".swp", ".tmp"];
            if noise_patterns.iter().any(|p| rel_str.contains(p)) {
                continue;
            }

            let lang_name = match extension_to_language(ext) {
                Some(l) => l,
                None => continue,
            };

            // Find matching parser
            if !self.parsers.iter().any(|(name, _)| name == lang_name) {
                continue;
            }

            let source = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(_) => continue,
            };

            let blocks = self.chunk_file(&source, path, root, lang_name)?;
            all_blocks.extend(blocks);
        }

        Ok(all_blocks)
    }

    /// Parse a single file and extract code blocks from its AST
    pub fn chunk_file(
        &mut self,
        source: &str,
        file_path: &Path,
        root: &Path,
        language: &str,
    ) -> Result<Vec<CodeBlock>> {
        let parser_idx = self.parsers.iter().position(|(name, _)| name == language);
        let parser_idx = match parser_idx {
            Some(i) => i,
            None => return Ok(Vec::new()),
        };
        let parser = &mut self.parsers[parser_idx].1;

        let tree = parser
            .parse(source, None)
            .context("Failed to parse source file")?;

        let root_node = tree.root_node();
        let cursor = &mut root_node.walk();
        let abs_path = file_path.to_path_buf();
        let path = pathdiff::diff_paths(file_path, root).unwrap_or_else(|| file_path.to_path_buf());

        let mut blocks = Vec::new();

        self.collect_blocks_recursive(
            cursor,
            source,
            &abs_path,
            &path,
            language,
            &mut blocks,
        );

        // Extract imports from file-level nodes
        let imports = extract_imports(source, language, &root_node);

        // Attach imports to all blocks from this file
        for block in &mut blocks {
            block.imports = imports.clone();
        }

        // If no blocks found, add whole file as a module block
        if blocks.is_empty() {
            let code = source.to_string();
            let sig_line = source.lines().next().unwrap_or("").to_string();
            blocks.push(CodeBlock {
                path: path.clone(),
                abs_path: abs_path.clone(),
                kind: BlockKind::Module,
                name: file_path
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default(),
                signature: sig_line,
                doc_comment: String::new(),
                code,
                language: language.to_string(),
                start_line: 1,
                end_line: source.lines().count(),
                imports,
            });
        }

        Ok(blocks)
    }

    /// Recursively walk AST tree and collect named code blocks.
    /// Uses the canonical tree-sitter cursor pattern: descend into the first
    /// child, recurse, walk siblings, then `goto_parent()` so the shared
    /// cursor returns to the parent. (The previous implementation omitted
    /// `goto_parent()`, which left the cursor stranded at a leaf and never
    /// visited sibling subtrees — so only whole-file blocks were ever found.)
    fn collect_blocks_recursive(
        &self,
        cursor: &mut tree_sitter::TreeCursor,
        source: &str,
        abs_path: &Path,
        path: &Path,
        language: &str,
        blocks: &mut Vec<CodeBlock>,
    ) {
        let node_types: &[&str] = match language {
            "rust" => &["function_item", "struct_item", "enum_item",
                         "trait_item", "impl_item", "type_item", "macro_definition"],
            "python" => &["function_definition", "class_definition", "async_function_definition"],
            "javascript" | "typescript" => &["function_declaration", "class_declaration",
                                               "method_definition", "arrow_function", "export_statement"],
            "go" => &["function_declaration", "method_declaration", "type_declaration",
                        "type_spec", "struct_type"],
            "java" => &["method_declaration", "class_declaration", "interface_declaration",
                          "enum_declaration", "constructor_declaration"],
            _ => &[],
        };

        // Descend into the first child; bail if none (leaf node).
        if !cursor.goto_first_child() {
            return;
        }
        loop {
            let node = cursor.node();
            let kind = node.kind();
            if node_types.iter().any(|t| *t == kind) {
                collect_single_block(
                    node, source, abs_path, path, language, blocks,
                );
            }

            // Recurse into this node's children before the next sibling.
            self.collect_blocks_recursive(
                cursor, source, abs_path, path, language, blocks,
            );

            if !cursor.goto_next_sibling() {
                break;
            }
        }
        // Return the shared cursor to the parent so callers can continue.
        cursor.goto_parent();
    }
}

/// Collect a single AST node as a code block
fn collect_single_block(
    node: tree_sitter::Node,
    source: &str,
    abs_path: &Path,
    path: &Path,
    language: &str,
    blocks: &mut Vec<CodeBlock>,
) {
    let kind_str = node.kind();
    let block_kind = map_kind(kind_str, language);

    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    let code_bytes = node.utf8_text(source.as_bytes()).unwrap_or("").to_string();

    // Get doc comment (lines above the node)
    let doc_comment = extract_doc_comment(source, node, language);

    // Get signature (first line of the node)
    let signature = code_bytes.lines().next().unwrap_or("").to_string();

    // Get name
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .unwrap_or("")
        .to_string();

    tracing::debug!(
        "Found block: {} {} at {}:{}-{}",
        block_kind_str(&block_kind), name, path.display(), start_line, end_line,
    );

    blocks.push(CodeBlock {
        path: path.to_path_buf(),
        abs_path: abs_path.to_path_buf(),
        kind: block_kind,
        name,
        signature,
        doc_comment,
        code: code_bytes,
        language: language.to_string(),
        start_line,
        end_line,
        imports: Vec::new(),
    });
}

fn map_kind(kind: &str, _language: &str) -> BlockKind {
    match kind {
        "function_item" | "function_definition" | "function_declaration" | "async_function_definition" => {
            BlockKind::Function
        }
        "class_definition" | "class_declaration" => BlockKind::Class,
        "struct_item" | "struct_type" | "type_spec" => BlockKind::Struct,
        "enum_item" | "enum_declaration" => BlockKind::Enum,
        "trait_item" | "interface_declaration" => BlockKind::Trait,
        "impl_item" => BlockKind::Impl,
        "method_definition" | "method_declaration" => BlockKind::Method,
        "type_item" | "type_declaration" | "type_alias" => BlockKind::Type,
        _ => BlockKind::Block,
    }
}

fn block_kind_str(kind: &BlockKind) -> &'static str {
    match kind {
        BlockKind::Function => "fn",
        BlockKind::Class => "class",
        BlockKind::Struct => "struct",
        BlockKind::Enum => "enum",
        BlockKind::Trait => "trait",
        BlockKind::Impl => "impl",
        BlockKind::Method => "method",
        BlockKind::Module => "module",
        BlockKind::Block => "block",
        BlockKind::Interface => "interface",
        BlockKind::Type => "type",
    }
}

/// Extract doc comments from lines preceding a node
fn extract_doc_comment(source: &str, node: tree_sitter::Node, language: &str) -> String {
    let start_line = node.start_position().row;
    if start_line == 0 {
        return String::new();
    }

    let lines: Vec<&str> = source.lines().collect();
    let mut doc_lines = Vec::new();

    // Go backwards from the node start to collect doc comments
    for i in (0..start_line).rev() {
        let line = lines[i].trim();
        match language {
            "rust" if line.starts_with("///") || line.starts_with("//!") => {
                doc_lines.push(line.trim_start_matches("///").trim_start_matches("//!").trim());
            }
            "python" if line.starts_with("\"\"\"") || line.starts_with("#") => {
                doc_lines.push(line.trim_start_matches('#').trim());
                if line.starts_with("\"\"\"") {
                    break;
                }
            }
            "javascript" | "typescript" | "java" if line.starts_with("/**") || line.starts_with("//") => {
                doc_lines.push(
                    line.trim_start_matches("/**")
                        .trim_start_matches("//")
                        .trim_end_matches("*/")
                        .trim(),
                );
                if line.starts_with("/**") {
                    break;
                }
            }
            _ => break,
        }
    }

    // Reverse to get original order
    doc_lines.reverse();
    doc_lines.join(" ")
}

/// Extract import statements from the file-level AST
fn extract_imports(source: &str, language: &str, root_node: &tree_sitter::Node) -> Vec<String> {
    let mut imports = Vec::new();
    let import_types = match language {
        "rust" => &["use_declaration"][..],
        "python" => &["import_statement", "import_from_statement"],
        "javascript" | "typescript" => &["import_statement", "import_require_clause"],
        "go" => &["import_declaration"],
        "java" => &["import_declaration", "package_declaration"],
        _ => return imports,
    };

    let mut cursor = root_node.walk();
    let mut children = cursor.goto_first_child();
    while children {
        let node = cursor.node();
        if import_types.contains(&node.kind()) {
            if let Ok(text) = node.utf8_text(source.as_bytes()) {
                imports.push(text.to_string());
            }
        }
        children = cursor.goto_next_sibling();
    }

    imports
}


