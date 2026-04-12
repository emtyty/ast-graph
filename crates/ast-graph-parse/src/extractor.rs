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
