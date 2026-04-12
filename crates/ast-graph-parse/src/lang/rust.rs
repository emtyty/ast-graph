use ast_graph_core::*;
use crate::extractor::*;
use std::path::Path;

pub struct RustExtractor;

impl LanguageExtractor for RustExtractor {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &tree_sitter::Tree, file_path: &Path) -> ExtractResult {
        let mut symbols = Vec::new();
        let mut raw_edges = Vec::new();
        let file_str = file_path.to_string_lossy();

        let file_node_id = NodeId::new(&file_str, &file_str, SymbolKind::File, 0);
        symbols.push(SymbolNode {
            id: file_node_id,
            name: file_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            kind: SymbolKind::File,
            file_path: file_path.to_path_buf(),
            line_range: (0, source.iter().filter(|&&b| b == b'\n').count() as u32),
            signature: None,
            doc_comment: None,
            visibility: Visibility::Public,
            language: Language::Rust,
            parent: None,
        });

        walk_node(
            source,
            &tree.root_node(),
            file_path,
            file_node_id,
            &mut symbols,
            &mut raw_edges,
        );

        ExtractResult { symbols, raw_edges }
    }
}

fn walk_node(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_item" => {
                if let Some(sym) = extract_function(source, &child, file_path, parent_id) {
                    let id = sym.id;
                    symbols.push(sym);
                    raw_edges.push(RawEdge {
                        source: parent_id,
                        kind: EdgeKind::Contains,
                        target_name: id.to_string(),
                        target_module: None,
                    });
                    extract_calls(source, &child, id, raw_edges);
                }
            }
            "struct_item" => {
                if let Some(sym) = extract_struct(source, &child, file_path, parent_id) {
                    let id = sym.id;
                    symbols.push(sym);
                    raw_edges.push(RawEdge {
                        source: parent_id,
                        kind: EdgeKind::Contains,
                        target_name: id.to_string(),
                        target_module: None,
                    });
                    extract_fields(source, &child, file_path, id, symbols, raw_edges);
                }
            }
            "enum_item" => {
                if let Some(sym) = extract_enum(source, &child, file_path, parent_id) {
                    symbols.push(sym);
                }
            }
            "trait_item" => {
                if let Some(sym) = extract_trait(source, &child, file_path, parent_id) {
                    let id = sym.id;
                    symbols.push(sym);
                    walk_node(source, &child, file_path, id, symbols, raw_edges);
                }
            }
            "impl_item" => {
                extract_impl(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "use_declaration" => {
                extract_use(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "mod_item" => {
                if let Some(sym) = extract_module(source, &child, file_path, parent_id) {
                    let id = sym.id;
                    symbols.push(sym);
                    walk_node(source, &child, file_path, id, symbols, raw_edges);
                }
            }
            "const_item" | "static_item" => {
                if let Some(name_node) = child_by_field(&child, "name") {
                    let name = node_text(source, &name_node);
                    let kind = if child.kind() == "const_item" {
                        SymbolKind::Constant
                    } else {
                        SymbolKind::Static
                    };
                    let id = NodeId::new(
                        &file_path.to_string_lossy(),
                        name,
                        kind,
                        child.start_position().row as u32,
                    );
                    symbols.push(SymbolNode {
                        id,
                        name: name.to_string(),
                        kind,
                        file_path: file_path.to_path_buf(),
                        line_range: (
                            child.start_position().row as u32,
                            child.end_position().row as u32,
                        ),
                        signature: Some(get_line_text(source, child.start_position().row)),
                        doc_comment: None,
                        visibility: extract_visibility(&child, source),
                        language: Language::Rust,
                        parent: Some(parent_id),
                    });
                }
            }
            "type_item" => {
                if let Some(name_node) = child_by_field(&child, "name") {
                    let name = node_text(source, &name_node);
                    let id = NodeId::new(
                        &file_path.to_string_lossy(),
                        name,
                        SymbolKind::TypeAlias,
                        child.start_position().row as u32,
                    );
                    symbols.push(SymbolNode {
                        id,
                        name: name.to_string(),
                        kind: SymbolKind::TypeAlias,
                        file_path: file_path.to_path_buf(),
                        line_range: (
                            child.start_position().row as u32,
                            child.end_position().row as u32,
                        ),
                        signature: Some(get_line_text(source, child.start_position().row)),
                        doc_comment: None,
                        visibility: extract_visibility(&child, source),
                        language: Language::Rust,
                        parent: Some(parent_id),
                    });
                }
            }
            _ => {
                // Recurse into other nodes to find nested items
                walk_node(source, &child, file_path, parent_id, symbols, raw_edges);
            }
        }
    }
}

fn extract_function(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
) -> Option<SymbolNode> {
    let name_node = child_by_field(node, "name")?;
    let name = node_text(source, &name_node);
    let file_str = file_path.to_string_lossy();

    let signature = build_fn_signature(source, node, name);

    Some(SymbolNode {
        id: NodeId::new(&file_str, name, SymbolKind::Function, node.start_position().row as u32),
        name: name.to_string(),
        kind: SymbolKind::Function,
        file_path: file_path.to_path_buf(),
        line_range: (
            node.start_position().row as u32,
            node.end_position().row as u32,
        ),
        signature: Some(signature),
        doc_comment: None,
        visibility: extract_visibility(node, source),
        language: Language::Rust,
        parent: Some(parent_id),
    })
}

fn extract_struct(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
) -> Option<SymbolNode> {
    let name_node = child_by_field(node, "name")?;
    let name = node_text(source, &name_node);

    Some(SymbolNode {
        id: NodeId::new(
            &file_path.to_string_lossy(),
            name,
            SymbolKind::Struct,
            node.start_position().row as u32,
        ),
        name: name.to_string(),
        kind: SymbolKind::Struct,
        file_path: file_path.to_path_buf(),
        line_range: (
            node.start_position().row as u32,
            node.end_position().row as u32,
        ),
        signature: Some(format!("struct {name}")),
        doc_comment: None,
        visibility: extract_visibility(node, source),
        language: Language::Rust,
        parent: Some(parent_id),
    })
}

fn extract_enum(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
) -> Option<SymbolNode> {
    let name_node = child_by_field(node, "name")?;
    let name = node_text(source, &name_node);

    Some(SymbolNode {
        id: NodeId::new(
            &file_path.to_string_lossy(),
            name,
            SymbolKind::Enum,
            node.start_position().row as u32,
        ),
        name: name.to_string(),
        kind: SymbolKind::Enum,
        file_path: file_path.to_path_buf(),
        line_range: (
            node.start_position().row as u32,
            node.end_position().row as u32,
        ),
        signature: Some(format!("enum {name}")),
        doc_comment: None,
        visibility: extract_visibility(node, source),
        language: Language::Rust,
        parent: Some(parent_id),
    })
}

fn extract_trait(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
) -> Option<SymbolNode> {
    let name_node = child_by_field(node, "name")?;
    let name = node_text(source, &name_node);

    Some(SymbolNode {
        id: NodeId::new(
            &file_path.to_string_lossy(),
            name,
            SymbolKind::Trait,
            node.start_position().row as u32,
        ),
        name: name.to_string(),
        kind: SymbolKind::Trait,
        file_path: file_path.to_path_buf(),
        line_range: (
            node.start_position().row as u32,
            node.end_position().row as u32,
        ),
        signature: Some(format!("trait {name}")),
        doc_comment: None,
        visibility: extract_visibility(node, source),
        language: Language::Rust,
        parent: Some(parent_id),
    })
}

fn extract_module(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
) -> Option<SymbolNode> {
    let name_node = child_by_field(node, "name")?;
    let name = node_text(source, &name_node);

    Some(SymbolNode {
        id: NodeId::new(
            &file_path.to_string_lossy(),
            name,
            SymbolKind::Module,
            node.start_position().row as u32,
        ),
        name: name.to_string(),
        kind: SymbolKind::Module,
        file_path: file_path.to_path_buf(),
        line_range: (
            node.start_position().row as u32,
            node.end_position().row as u32,
        ),
        signature: Some(format!("mod {name}")),
        doc_comment: None,
        visibility: extract_visibility(node, source),
        language: Language::Rust,
        parent: Some(parent_id),
    })
}

fn extract_impl(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    // Get the type being implemented
    let type_node = match child_by_field(node, "type") {
        Some(n) => n,
        None => return,
    };
    let type_name = node_text(source, &type_node);

    // Check if this is a trait impl
    let trait_name = child_by_field(node, "trait").map(|n| node_text(source, &n).to_string());

    if let Some(ref trait_name) = trait_name {
        raw_edges.push(RawEdge {
            source: NodeId::new(
                &file_path.to_string_lossy(),
                type_name,
                SymbolKind::Struct,
                0,
            ),
            kind: EdgeKind::Implements,
            target_name: trait_name.clone(),
            target_module: None,
        });
    }

    // Extract methods inside the impl block
    if let Some(body) = find_child_by_kind(node, "declaration_list") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "function_item" {
                if let Some(name_node) = child_by_field(&child, "name") {
                    let name = node_text(source, &name_node);
                    let method_id = NodeId::new(
                        &file_path.to_string_lossy(),
                        &format!("{type_name}::{name}"),
                        SymbolKind::Method,
                        child.start_position().row as u32,
                    );

                    let signature = build_fn_signature(source, &child, name);

                    symbols.push(SymbolNode {
                        id: method_id,
                        name: format!("{type_name}::{name}"),
                        kind: SymbolKind::Method,
                        file_path: file_path.to_path_buf(),
                        line_range: (
                            child.start_position().row as u32,
                            child.end_position().row as u32,
                        ),
                        signature: Some(signature),
                        doc_comment: None,
                        visibility: extract_visibility(&child, source),
                        language: Language::Rust,
                        parent: Some(parent_id),
                    });

                    extract_calls(source, &child, method_id, raw_edges);
                }
            }
        }
    }
}

fn extract_use(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let use_text = node_text(source, node).trim().to_string();
    // Extract the path from "use foo::bar::Baz;"
    let import_path = use_text
        .strip_prefix("pub ")
        .unwrap_or(&use_text)
        .strip_prefix("use ")
        .unwrap_or(&use_text)
        .trim_end_matches(';')
        .to_string();

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &import_path,
        SymbolKind::Import,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: import_path.clone(),
        kind: SymbolKind::Import,
        file_path: file_path.to_path_buf(),
        line_range: (
            node.start_position().row as u32,
            node.end_position().row as u32,
        ),
        signature: Some(use_text),
        doc_comment: None,
        visibility: extract_visibility(node, source),
        language: Language::Rust,
        parent: Some(parent_id),
    });

    raw_edges.push(RawEdge {
        source: parent_id,
        kind: EdgeKind::Imports,
        target_name: import_path,
        target_module: None,
    });
}

fn extract_fields(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    if let Some(body) = find_child_by_kind(node, "field_declaration_list") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "field_declaration" {
                if let Some(name_node) = child_by_field(&child, "name") {
                    let name = node_text(source, &name_node);
                    let type_text = child_by_field(&child, "type")
                        .map(|t| node_text(source, &t).to_string());

                    let id = NodeId::new(
                        &file_path.to_string_lossy(),
                        &format!("{}.{name}", parent_id),
                        SymbolKind::Field,
                        child.start_position().row as u32,
                    );

                    symbols.push(SymbolNode {
                        id,
                        name: name.to_string(),
                        kind: SymbolKind::Field,
                        file_path: file_path.to_path_buf(),
                        line_range: (
                            child.start_position().row as u32,
                            child.end_position().row as u32,
                        ),
                        signature: type_text.clone(),
                        doc_comment: None,
                        visibility: extract_visibility(&child, source),
                        language: Language::Rust,
                        parent: Some(parent_id),
                    });

                    // Add type reference edge
                    if let Some(type_name) = type_text {
                        raw_edges.push(RawEdge {
                            source: id,
                            kind: EdgeKind::References,
                            target_name: type_name,
                            target_module: None,
                        });
                    }
                }
            }
        }
    }
}

fn extract_calls(
    source: &[u8],
    node: &tree_sitter::Node,
    parent_id: NodeId,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            if let Some(func) = child_by_field(&child, "function") {
                let call_target = node_text(source, &func);
                raw_edges.push(RawEdge {
                    source: parent_id,
                    kind: EdgeKind::Calls,
                    target_name: call_target.to_string(),
                    target_module: None,
                });
            }
        }
        // Recurse to find nested calls
        extract_calls(source, &child, parent_id, raw_edges);
    }
}

fn extract_visibility(node: &tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            let text = node_text(source, &child);
            if text.starts_with("pub") {
                return Visibility::Public;
            }
        }
    }
    Visibility::Private
}

fn build_fn_signature(source: &[u8], node: &tree_sitter::Node, name: &str) -> String {
    let params = child_by_field(node, "parameters")
        .map(|p| node_text(source, &p).to_string())
        .unwrap_or_else(|| "()".to_string());

    let return_type = child_by_field(node, "return_type")
        .map(|r| format!(" -> {}", node_text(source, &r)))
        .unwrap_or_default();

    let vis = if extract_visibility(node, source) == Visibility::Public {
        "pub "
    } else {
        ""
    };

    format!("{vis}fn {name}{params}{return_type}")
}

fn get_line_text(source: &[u8], line: usize) -> String {
    let text = String::from_utf8_lossy(source);
    text.lines()
        .nth(line)
        .unwrap_or("")
        .trim()
        .to_string()
}
