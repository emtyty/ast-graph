use ast_graph_core::*;
use crate::extractor::*;
use std::path::Path;

pub struct JavaExtractor;

impl LanguageExtractor for JavaExtractor {
    fn language(&self) -> Language {
        Language::Java
    }

    fn file_extensions(&self) -> &[&str] {
        &["java"]
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_java::LANGUAGE.into()
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
            language: Language::Java,
            parent: None,
        });

        walk_java(source, &tree.root_node(), file_path, file_node_id, None, &mut symbols, &mut raw_edges);
        ExtractResult { symbols, raw_edges }
    }
}

fn walk_java(
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
            "package_declaration" => {
                extract_java_package(source, &child, file_path, parent_id, symbols);
            }
            "import_declaration" => {
                extract_java_import(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "class_declaration" => {
                extract_java_class(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "interface_declaration" => {
                extract_java_interface(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "enum_declaration" => {
                extract_java_enum(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "record_declaration" => {
                extract_java_record(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "annotation_type_declaration" => {
                extract_java_annotation_type(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "method_declaration" => {
                extract_java_method(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
            "constructor_declaration" => {
                extract_java_constructor(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
            "field_declaration" => {
                extract_java_field(source, &child, file_path, parent_id, symbols);
            }
            _ => {
                walk_java(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
        }
    }
}

fn extract_java_package(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
) {
    // package_declaration: package <scoped_identifier> ;
    // Find the identifier/scoped_identifier child
    let mut cursor = node.walk();
    let name = node
        .children(&mut cursor)
        .find(|c| c.kind() == "scoped_identifier" || c.kind() == "identifier")
        .map(|c| node_text(source, &c).to_string())
        .unwrap_or_default();

    if name.is_empty() {
        return;
    }

    let id = NodeId::new(
        &file_path.to_string_lossy(), &name, SymbolKind::Package,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.clone(),
        kind: SymbolKind::Package,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("package {name}")),
        doc_comment: None,
        visibility: Visibility::Public,
        language: Language::Java,
        parent: Some(parent_id),
    });
}

fn extract_java_import(
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
        language: Language::Java,
        parent: Some(parent_id),
    });

    raw_edges.push(RawEdge {
        source: parent_id,
        kind: EdgeKind::Imports,
        target_name: text,
        target_module: None,
        source_line: node.start_position().row as u32,
    });
}

fn extract_java_class(
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
        doc_comment: None,
        visibility: extract_java_visibility(node, source),
        language: Language::Java,
        parent: Some(parent_id),
    });

    // superclass field: extends <type>
    if let Some(superclass) = child_by_field(node, "superclass") {
        for base in named_type_names(source, &superclass) {
            raw_edges.push(RawEdge {
                source: id,
                kind: EdgeKind::Extends,
                target_name: base,
                target_module: None,
                source_line: superclass.start_position().row as u32,
            });
        }
    }

    // interfaces field: implements <type_list>
    if let Some(interfaces) = child_by_field(node, "interfaces") {
        for iface in named_type_names(source, &interfaces) {
            raw_edges.push(RawEdge {
                source: id,
                kind: EdgeKind::Implements,
                target_name: iface,
                target_module: None,
                source_line: interfaces.start_position().row as u32,
            });
        }
    }

    if let Some(body) = child_by_field(node, "body") {
        walk_java(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_java_interface(
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
        doc_comment: None,
        visibility: extract_java_visibility(node, source),
        language: Language::Java,
        parent: Some(parent_id),
    });

    // Interface inheritance: extends <type_list>
    if let Some(extends) = child_by_field(node, "extends_interfaces") {
        for base in named_type_names(source, &extends) {
            raw_edges.push(RawEdge {
                source: id,
                kind: EdgeKind::Extends,
                target_name: base,
                target_module: None,
                source_line: extends.start_position().row as u32,
            });
        }
    }

    if let Some(body) = child_by_field(node, "body") {
        walk_java(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_java_enum(
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
        &file_path.to_string_lossy(), name, SymbolKind::Enum,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Enum,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("enum {name}")),
        doc_comment: None,
        visibility: extract_java_visibility(node, source),
        language: Language::Java,
        parent: Some(parent_id),
    });

    if let Some(interfaces) = child_by_field(node, "interfaces") {
        for iface in named_type_names(source, &interfaces) {
            raw_edges.push(RawEdge {
                source: id,
                kind: EdgeKind::Implements,
                target_name: iface,
                target_module: None,
                source_line: interfaces.start_position().row as u32,
            });
        }
    }

    if let Some(body) = child_by_field(node, "body") {
        // Capture enum constants as EnumVariant
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "enum_constant" {
                if let Some(ec_name) = child_by_field(&child, "name") {
                    let ec = node_text(source, &ec_name);
                    let ec_id = NodeId::new(
                        &file_path.to_string_lossy(), ec, SymbolKind::EnumVariant,
                        child.start_position().row as u32,
                    );
                    symbols.push(SymbolNode {
                        id: ec_id,
                        name: ec.to_string(),
                        kind: SymbolKind::EnumVariant,
                        file_path: file_path.to_path_buf(),
                        line_range: (child.start_position().row as u32, child.end_position().row as u32),
                        signature: None,
                        doc_comment: None,
                        visibility: Visibility::Public,
                        language: Language::Java,
                        parent: Some(id),
                    });
                }
            }
        }
        walk_java(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_java_record(
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
        doc_comment: None,
        visibility: extract_java_visibility(node, source),
        language: Language::Java,
        parent: Some(parent_id),
    });

    if let Some(interfaces) = child_by_field(node, "interfaces") {
        for iface in named_type_names(source, &interfaces) {
            raw_edges.push(RawEdge {
                source: id,
                kind: EdgeKind::Implements,
                target_name: iface,
                target_module: None,
                source_line: interfaces.start_position().row as u32,
            });
        }
    }

    if let Some(body) = child_by_field(node, "body") {
        walk_java(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_java_annotation_type(
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
        signature: Some(format!("@interface {name}")),
        doc_comment: None,
        visibility: extract_java_visibility(node, source),
        language: Language::Java,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        walk_java(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_java_method(
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
        doc_comment: None,
        visibility: extract_java_visibility(node, source),
        language: Language::Java,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        extract_java_calls(source, &body, id, class_name, raw_edges);
    }
}

fn extract_java_constructor(
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
        doc_comment: None,
        visibility: extract_java_visibility(node, source),
        language: Language::Java,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        extract_java_calls(source, &body, id, class_name, raw_edges);
    }
}

fn extract_java_field(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
) {
    let field_type = child_by_field(node, "type")
        .map(|t| node_text(source, &t).to_string())
        .unwrap_or_default();

    // A field_declaration may declare multiple variables: `int a, b = 2;`
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let var_name_node = match child_by_field(&child, "name") {
                Some(n) => n,
                None => continue,
            };
            let var_name = node_text(source, &var_name_node);

            let id = NodeId::new(
                &file_path.to_string_lossy(), var_name, SymbolKind::Field,
                node.start_position().row as u32,
            );

            symbols.push(SymbolNode {
                id,
                name: var_name.to_string(),
                kind: SymbolKind::Field,
                file_path: file_path.to_path_buf(),
                line_range: (node.start_position().row as u32, node.end_position().row as u32),
                signature: Some(format!("{field_type} {var_name}")),
                doc_comment: None,
                visibility: extract_java_visibility(node, source),
                language: Language::Java,
                parent: Some(parent_id),
            });
        }
    }
}

fn extract_java_calls(
    source: &[u8],
    node: &tree_sitter::Node,
    parent_id: NodeId,
    class_name: Option<&str>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "method_invocation" => {
                // Java method_invocation: [object .] name ( arguments )
                let name = child_by_field(&child, "name")
                    .map(|n| node_text(source, &n).to_string())
                    .unwrap_or_default();
                if !name.is_empty() {
                    let raw_target = match child_by_field(&child, "object") {
                        Some(obj) => format!("{}.{}", node_text(source, &obj), name),
                        None => name,
                    };
                    let call_target = match class_name {
                        Some(cn) => crate::extractor::qualify_member_call(&raw_target, cn),
                        None => raw_target,
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
            "object_creation_expression" => {
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
            _ => {}
        }
        extract_java_calls(source, &child, parent_id, class_name, raw_edges);
    }
}

fn extract_java_visibility(node: &tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            let mut mod_cursor = child.walk();
            for m in child.children(&mut mod_cursor) {
                match m.kind() {
                    "public" => return Visibility::Public,
                    "private" => return Visibility::Private,
                    "protected" => return Visibility::Protected,
                    _ => {
                        // tree-sitter-java sometimes emits modifier tokens as their literal kind,
                        // but older grammars surface them as plain identifiers. Match on text too.
                        let text = node_text(source, &m);
                        match text {
                            "public" => return Visibility::Public,
                            "private" => return Visibility::Private,
                            "protected" => return Visibility::Protected,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    // Java default: package-private
    Visibility::Internal
}

/// Extract named type identifiers from a `superclass`, `interfaces`, or
/// `extends_interfaces` node. These wrappers contain one or more types
/// (type_identifier, generic_type, scoped_type_identifier).
fn named_type_names(source: &[u8], node: &tree_sitter::Node) -> Vec<String> {
    let mut out = Vec::new();
    collect_type_names(source, node, &mut out);
    out
}

fn collect_type_names(source: &[u8], node: &tree_sitter::Node, out: &mut Vec<String>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "type_identifier" => {
                out.push(node_text(source, &child).to_string());
            }
            "generic_type" => {
                // generic_type wraps a type_identifier + type_arguments
                if let Some(name) = child_by_field(&child, "name") {
                    out.push(node_text(source, &name).to_string());
                } else if let Some(t) = find_child_by_kind(&child, "type_identifier") {
                    out.push(node_text(source, &t).to_string());
                }
            }
            "scoped_type_identifier" => {
                // Use the full scoped name (e.g. java.util.List)
                out.push(node_text(source, &child).to_string());
            }
            _ => {
                collect_type_names(source, &child, out);
            }
        }
    }
}
