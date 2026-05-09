use ast_graph_core::{Language, RawEdge, SymbolNode};
use std::path::Path;

/// Result of extracting symbols from a single file.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    pub symbols: Vec<SymbolNode>,
    pub raw_edges: Vec<RawEdge>,
}

/// Trait implemented by each language extractor.
/// Walks the tree-sitter CST and extracts only structural/semantic nodes.
pub trait LanguageExtractor: Send + Sync {
    fn language(&self) -> Language;

    fn file_extensions(&self) -> &[&str];

    fn tree_sitter_language(&self) -> tree_sitter::Language;

    /// Extract compressed symbols and raw (unresolved) edges from a parsed tree.
    fn extract(
        &self,
        source: &[u8],
        tree: &tree_sitter::Tree,
        file_path: &Path,
    ) -> ExtractResult;
}

/// Helper to get text for a tree-sitter node from source bytes.
pub fn node_text<'a>(source: &'a [u8], node: &tree_sitter::Node) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// Helper to find the first child of a given kind.
pub fn find_child_by_kind<'a>(
    node: &'a tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == kind {
            return Some(child);
        }
    }
    None
}

/// Helper to find the first child whose `kind()` is in the given list.
/// Useful when multiple grammar versions or sibling node kinds need to match.
pub fn find_child_by_any_kind<'a>(
    node: &'a tree_sitter::Node<'a>,
    kinds: &[&str],
) -> Option<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if kinds.contains(&child.kind()) {
            return Some(child);
        }
    }
    None
}

/// Helper to find a named child by field name.
pub fn child_by_field<'a>(
    node: &'a tree_sitter::Node<'a>,
    field: &str,
) -> Option<tree_sitter::Node<'a>> {
    node.child_by_field_name(field)
}

/// Qualify a `this.method` or `self.method` call target with the enclosing class name.
/// Only qualifies single-level member access to avoid false matches on chained calls.
/// Examples:
///   qualify_member_call("this.save", "ClassName")  -> "ClassName.save"
///   qualify_member_call("self.save", "ClassName")  -> "ClassName.save"
///   qualify_member_call("this.a.b", "ClassName")   -> "this.a.b"  (multi-level, unchanged)
///   qualify_member_call("otherObj.foo", "ClassName") -> "otherObj.foo" (no this/self prefix)
pub fn qualify_member_call(target: &str, class_name: &str) -> String {
    let method = if let Some(m) = target.strip_prefix("this.") {
        m
    } else if let Some(m) = target.strip_prefix("self.") {
        m
    } else {
        return target.to_string();
    };
    // Only qualify single-level: "save" not "obj.save"
    if method.contains('.') {
        return target.to_string();
    }
    format!("{class_name}.{method}")
}

/// Walk back from `node` collecting adjacent preceding comment siblings.
/// Comments are considered "doc comments" when each one ends within one line
/// of the next thing it precedes (no blank-line gap).  Returns the joined
/// comment text with leading comment markers stripped, or `None` if none found.
///
/// Suitable for: Rust (`///`, `//!`, `/** */`), Go (`//`), Java/C# (`/** */`,
/// `///`), JavaScript/TypeScript (JSDoc `/** */`).  Not used for Python
/// (use `extract_python_docstring` for `"""..."""` docstrings instead).
pub fn extract_preceding_doc_comment(source: &[u8], node: &tree_sitter::Node) -> Option<String> {
    let mut comments: Vec<String> = Vec::new();
    let mut current_top_byte = node.start_byte();
    let mut prev = node.prev_sibling();

    while let Some(p) = prev {
        if !is_comment_node(p.kind()) {
            break;
        }
        // Count newlines in the byte gap between this comment's end and the
        // current top of the doc-comment chain.  A single newline is just the
        // line ending; two or more means there's a blank line and the doc
        // comment isn't actually attached to this declaration.
        let end_byte = p.end_byte();
        if end_byte > current_top_byte {
            // Defensive — shouldn't happen for prev_siblings, but skip if it does.
            break;
        }
        // Whether tree-sitter includes the comment's trailing newline in its
        // byte range varies by grammar.  Compute a per-grammar-correct cap on
        // newlines allowed in the gap: if the comment text ends with a `\n`,
        // it already "owns" its line-end (cap = 0); otherwise the line-end
        // lives in the gap (cap = 1).  Anything above that cap is a blank
        // line and breaks the doc-comment chain.
        let raw = node_text(source, &p);
        let owns_line_end = raw.ends_with('\n');
        let max_gap_newlines = if owns_line_end { 0 } else { 1 };
        let gap = &source[end_byte..current_top_byte];
        let gap_newlines = gap.iter().filter(|&&b| b == b'\n').count();
        if gap_newlines > max_gap_newlines {
            break;
        }
        comments.push(strip_comment_markers(node_text(source, &p)));
        current_top_byte = p.start_byte();
        prev = p.prev_sibling();
    }

    if comments.is_empty() {
        None
    } else {
        comments.reverse();
        let joined = comments.join("\n").trim().to_string();
        if joined.is_empty() { None } else { Some(joined) }
    }
}

fn is_comment_node(kind: &str) -> bool {
    matches!(
        kind,
        "comment" | "line_comment" | "block_comment" | "doc_comment"
    )
}

fn strip_comment_markers(raw: &str) -> String {
    let trimmed = raw.trim();
    // Block comments: /** ... */, /* ... */
    if let Some(inner) = trimmed.strip_prefix("/**").and_then(|s| s.strip_suffix("*/")) {
        return clean_block_comment(inner);
    }
    if let Some(inner) = trimmed.strip_prefix("/*").and_then(|s| s.strip_suffix("*/")) {
        return clean_block_comment(inner);
    }
    // Line comments: ///, //!, //
    if let Some(rest) = trimmed.strip_prefix("///") {
        return rest.trim().to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("//!") {
        return rest.trim().to_string();
    }
    if let Some(rest) = trimmed.strip_prefix("//") {
        return rest.trim().to_string();
    }
    // Python-style # comments (not used for docstrings, but cheap to support)
    if let Some(rest) = trimmed.strip_prefix('#') {
        return rest.trim().to_string();
    }
    trimmed.to_string()
}

fn clean_block_comment(inner: &str) -> String {
    inner
        .lines()
        .map(|line| line.trim().trim_start_matches('*').trim())
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Same as `extract_preceding_doc_comment`, but if the node has no preceding
/// comment of its own, walk up one level to a known "wrapper" parent and try
/// there.  Handles cases like Go's `type_declaration` wrapping `type_spec`,
/// or Python's `decorated_definition` wrapping `function_definition`.
pub fn extract_doc_comment_anchor(source: &[u8], node: &tree_sitter::Node) -> Option<String> {
    if let Some(c) = extract_preceding_doc_comment(source, node) {
        return Some(c);
    }
    let parent = node.parent()?;
    if matches!(
        parent.kind(),
        "type_declaration" | "decorated_definition" | "const_declaration" | "var_declaration"
    ) {
        extract_preceding_doc_comment(source, &parent)
    } else {
        None
    }
}

/// Extract a Python docstring from a function/class body node — the first
/// statement of the body, if it's a bare string expression.
pub fn extract_python_docstring(source: &[u8], body_node: &tree_sitter::Node) -> Option<String> {
    let mut cursor = body_node.walk();
    let first_named = body_node.children(&mut cursor).find(|n| n.is_named())?;
    if first_named.kind() != "expression_statement" {
        return None;
    }
    let mut c2 = first_named.walk();
    let inner = first_named.children(&mut c2).find(|n| n.is_named())?;
    if inner.kind() != "string" {
        return None;
    }
    let raw = node_text(source, &inner).trim();
    // Strip triple or single quotes (with optional r/b/f prefixes).
    let stripped = raw
        .trim_start_matches(|c: char| matches!(c, 'r' | 'R' | 'b' | 'B' | 'f' | 'F' | 'u' | 'U'))
        .trim_matches(|c: char| c == '"' || c == '\'')
        .trim()
        .to_string();
    if stripped.is_empty() { None } else { Some(stripped) }
}
