use ast_graph_core::*;
use crate::extractor::*;
use std::path::Path;

pub struct CSharpExtractor;

impl LanguageExtractor for CSharpExtractor {
    fn language(&self) -> Language {
        Language::CSharp
    }

    fn file_extensions(&self) -> &[&str] {
        &["cs"]
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_c_sharp::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &tree_sitter::Tree, file_path: &Path) -> ExtractResult {
        let mut symbols = Vec::new();
        let mut raw_edges = Vec::new();
        let file_str = file_path.to_string_lossy();

        let file_node_id = NodeId::new(&file_str, &file_str, SymbolKind::File, 0);
        symbols.push(SymbolNode {
            id: file_node_id,
            name: file_path.file_name().unwrap_or_default().to_string_lossy().to_string(),
            kind: SymbolKind::File,
            file_path: file_path.to_path_buf(),
            line_range: (0, source.iter().filter(|&&b| b == b'\n').count() as u32),
            signature: None,
            doc_comment: None,
            visibility: Visibility::Public,
            language: Language::CSharp,
            parent: None,
        });

        walk_csharp(source, &tree.root_node(), file_path, file_node_id, None, &mut symbols, &mut raw_edges);
        ExtractResult { symbols, raw_edges }
    }
}

fn walk_csharp(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    class_name: Option<&str>,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "namespace_declaration" | "file_scoped_namespace_declaration" => {
                extract_cs_namespace(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "class_declaration" => {
                extract_cs_class(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "struct_declaration" => {
                extract_cs_struct(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "interface_declaration" => {
                extract_cs_interface(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "enum_declaration" => {
                if let Some(name_node) = child_by_field(&child, "name") {
                    let name = node_text(source, &name_node);
                    let id = NodeId::new(
                        &file_path.to_string_lossy(), name, SymbolKind::Enum,
                        child.start_position().row as u32,
                    );
                    symbols.push(SymbolNode {
                        id,
                        name: name.to_string(),
                        kind: SymbolKind::Enum,
                        file_path: file_path.to_path_buf(),
                        line_range: (child.start_position().row as u32, child.end_position().row as u32),
                        signature: Some(format!("enum {name}")),
                        doc_comment: extract_preceding_doc_comment(source, &child),
                        visibility: extract_cs_visibility(&child, source),
                        language: Language::CSharp,
                        parent: Some(parent_id),
                    });
                }
            }
            "record_declaration" => {
                extract_cs_record(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "using_directive" => {
                extract_cs_using(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "method_declaration" => {
                extract_cs_method(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
            "constructor_declaration" => {
                extract_cs_constructor(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
            "property_declaration" => {
                extract_cs_property(source, &child, file_path, parent_id, symbols);
            }
            _ => {
                walk_csharp(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
        }
    }
}

fn extract_cs_namespace(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, SymbolKind::Namespace,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Namespace,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("namespace {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: Visibility::Public,
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    // Recurse into namespace body
    if let Some(body) = child_by_field(node, "body") {
        walk_csharp(source, &body, file_path, id, None, symbols, raw_edges);
    }
    // File-scoped namespaces don't have a body node; their children are siblings
    walk_csharp(source, node, file_path, id, None, symbols, raw_edges);
}

fn extract_cs_class(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, SymbolKind::Class,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Class,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("class {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_cs_visibility(node, source),
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    // Extract base types
    if let Some(bases) = child_by_field(node, "bases") {
        let mut cursor = bases.walk();
        for base_child in bases.children(&mut cursor) {
            if base_child.is_named() {
                let base_name = node_text(source, &base_child);
                // Heuristic: interfaces start with 'I' in C#
                let edge_kind = if base_name.starts_with('I') && base_name.len() > 1 && base_name.chars().nth(1).map_or(false, |c| c.is_uppercase()) {
                    EdgeKind::Implements
                } else {
                    EdgeKind::Extends
                };
                raw_edges.push(RawEdge {
                    source: id,
                    kind: edge_kind,
                    target_name: base_name.to_string(),
                    target_module: None,
                    source_line: base_child.start_position().row as u32,
                });
            }
        }
    }

    // Recurse into class body — pass class name so methods get qualified names
    if let Some(body) = child_by_field(node, "body") {
        walk_csharp(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_cs_struct(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, SymbolKind::Struct,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Struct,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("struct {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_cs_visibility(node, source),
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        walk_csharp(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_cs_interface(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, SymbolKind::Interface,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Interface,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("interface {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_cs_visibility(node, source),
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        walk_csharp(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_cs_record(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, SymbolKind::Record,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Record,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("record {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_cs_visibility(node, source),
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        walk_csharp(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_cs_method(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    class_name: Option<&str>,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let return_type = child_by_field(node, "type")
        .map(|t| node_text(source, &t).to_string())
        .unwrap_or_else(|| "void".to_string());

    let params = child_by_field(node, "parameters")
        .map(|p| node_text(source, &p).to_string())
        .unwrap_or_else(|| "()".to_string());

    // Qualify the method name with its enclosing class for precise resolution
    let qualified_name = match class_name {
        Some(cn) => format!("{cn}.{name}"),
        None => name.to_string(),
    };

    let id = NodeId::new(
        &file_path.to_string_lossy(), &qualified_name, SymbolKind::Method,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: qualified_name,
        kind: SymbolKind::Method,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("{return_type} {name}{params}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_cs_visibility(node, source),
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    // Extract calls from method body
    if let Some(body) = child_by_field(node, "body") {
        extract_cs_calls(source, &body, id, class_name, raw_edges);
    }
}

fn extract_cs_constructor(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    class_name: Option<&str>,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let params = child_by_field(node, "parameters")
        .map(|p| node_text(source, &p).to_string())
        .unwrap_or_else(|| "()".to_string());

    // Constructors use the class name as their node name; qualify to "ClassName..ctor" convention
    let qualified_name = match class_name {
        Some(cn) => format!("{cn}..ctor"),
        None => name.to_string(),
    };

    let id = NodeId::new(
        &file_path.to_string_lossy(), &qualified_name, SymbolKind::Constructor,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: qualified_name,
        kind: SymbolKind::Constructor,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("{name}{params}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_cs_visibility(node, source),
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        extract_cs_calls(source, &body, id, class_name, raw_edges);
    }
}

fn extract_cs_property(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let prop_type = child_by_field(node, "type")
        .map(|t| node_text(source, &t).to_string())
        .unwrap_or_default();

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, SymbolKind::Property,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Property,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("{prop_type} {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_cs_visibility(node, source),
        language: Language::CSharp,
        parent: Some(parent_id),
    });
}

fn extract_cs_using(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let text = node_text(source, node).trim().to_string();

    let id = NodeId::new(
        &file_path.to_string_lossy(), &text, SymbolKind::Import,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: text.clone(),
        kind: SymbolKind::Import,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(text.clone()),
        doc_comment: None,
        visibility: Visibility::Public,
        language: Language::CSharp,
        parent: Some(parent_id),
    });

    // Use the Import node's own id as the edge target so the resolver links
    // this file's :IMPORTS to *its* Import node, not to every file-wide Import
    // whose `name` happens to match (e.g. identical `using System;` lines across
    // the project). Same hex-NodeId convention that EdgeKind::Contains uses.
    raw_edges.push(RawEdge {
        source: parent_id,
        kind: EdgeKind::Imports,
        target_name: id.to_string(),
        target_module: None,
        source_line: node.start_position().row as u32,
    });
}

fn extract_cs_calls(
    source: &[u8],
    node: &tree_sitter::Node,
    parent_id: NodeId,
    class_name: Option<&str>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "invocation_expression" {
            if let Some(func) = child_by_field(&child, "function") {
                let raw_target = node_text(source, &func);
                let call_target = match class_name {
                    Some(cn) => crate::extractor::qualify_member_call(raw_target, cn),
                    None => raw_target.to_string(),
                };
                raw_edges.push(RawEdge {
                    source: parent_id,
                    kind: EdgeKind::Calls,
                    target_name: call_target,
                    target_module: None,
                    source_line: child.start_position().row as u32,
                });
            }
        }
        if child.kind() == "object_creation_expression" {
            if let Some(type_node) = child_by_field(&child, "type") {
                let type_name = node_text(source, &type_node);
                raw_edges.push(RawEdge {
                    source: parent_id,
                    kind: EdgeKind::References,
                    target_name: type_name.to_string(),
                    target_module: None,
                    source_line: child.start_position().row as u32,
                });
            }
        }
        extract_cs_calls(source, &child, parent_id, class_name, raw_edges);
    }
}

fn extract_cs_visibility(node: &tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifier" {
            let text = node_text(source, &child);
            match text {
                "public" => return Visibility::Public,
                "private" => return Visibility::Private,
                "protected" => return Visibility::Protected,
                "internal" => return Visibility::Internal,
                _ => {}
            }
        }
    }
    Visibility::Private // C# default
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::LanguageExtractor;

    fn extract(src: &str) -> (Vec<SymbolNode>, Vec<RawEdge>) {
        let extractor = CSharpExtractor;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&extractor.tree_sitter_language()).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        let r = extractor.extract(src.as_bytes(), &tree, Path::new("test.cs"));
        (r.symbols, r.raw_edges)
    }

    fn find<'a>(syms: &'a [SymbolNode], name: &str) -> Option<&'a SymbolNode> {
        syms.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_class_and_method() {
        let src = "namespace App { public class Foo { public int Bar() { return 42; } } }\n";
        let (syms, _) = extract(src);
        assert!(find(&syms, "Foo").is_some());
        let m = find(&syms, "Foo.Bar").expect("Foo.Bar missing");
        assert_eq!(m.kind, SymbolKind::Method);
        assert_eq!(m.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_interface() {
        let src = "namespace App { public interface IFoo { void Bar(); } }\n";
        let (syms, _) = extract(src);
        assert_eq!(find(&syms, "IFoo").map(|s| s.kind), Some(SymbolKind::Interface));
    }

    #[test]
    fn extracts_struct_and_record() {
        let src = "namespace App { public struct Point { } public record User(string Name); }\n";
        let (syms, _) = extract(src);
        assert_eq!(find(&syms, "Point").map(|s| s.kind), Some(SymbolKind::Struct));
        assert_eq!(find(&syms, "User").map(|s| s.kind), Some(SymbolKind::Record));
    }

    #[test]
    fn extracts_enum() {
        let src = "namespace App { public enum Color { Red, Green, Blue } }\n";
        let (syms, _) = extract(src);
        assert_eq!(find(&syms, "Color").map(|s| s.kind), Some(SymbolKind::Enum));
    }

    #[test]
    fn extracts_namespace() {
        let src = "namespace MyApp.Services { public class Foo { } }\n";
        let (syms, _) = extract(src);
        let ns = syms.iter().find(|s| s.kind == SymbolKind::Namespace);
        assert!(ns.is_some(), "expected a Namespace symbol");
    }

    #[test]
    fn extracts_xml_doc_comment() {
        let src = "namespace App {\n    /// <summary>Adds two numbers.</summary>\n    public class Foo {\n        public int Bar() { return 42; }\n    }\n}\n";
        let (syms, _) = extract(src);
        let f = find(&syms, "Foo").expect("Foo missing");
        let doc = f.doc_comment.as_deref().unwrap_or("");
        assert!(doc.contains("Adds two numbers") || doc.contains("<summary>"), "doc was: {doc:?}");
    }
}
