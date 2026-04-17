use ast_graph_core::*;
use crate::extractor::*;
use std::path::Path;

pub struct JavaScriptExtractor {
    language: Language,
}

impl JavaScriptExtractor {
    pub fn new(language: Language) -> Self {
        Self { language }
    }
}

impl LanguageExtractor for JavaScriptExtractor {
    fn language(&self) -> Language {
        self.language
    }

    fn file_extensions(&self) -> &[&str] {
        match self.language {
            Language::TypeScript => &["ts", "tsx"],
            _ => &["js", "jsx", "mjs", "cjs"],
        }
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        match self.language {
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TSX.into(),
            _ => tree_sitter_typescript::LANGUAGE_TSX.into(), // TSX parser handles JS too
        }
    }

    fn extract(&self, source: &[u8], tree: &tree_sitter::Tree, file_path: &Path) -> ExtractResult {
        let mut symbols = Vec::new();
        let mut raw_edges = Vec::new();
        let file_str = file_path.to_string_lossy();
        let lang = self.language;

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
            language: lang,
            parent: None,
        });

        walk_js(source, &tree.root_node(), file_path, file_node_id, lang, &mut symbols, &mut raw_edges);
        ExtractResult { symbols, raw_edges }
    }
}

fn walk_js(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    lang: Language,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_declaration" => {
                extract_js_function(source, &child, file_path, parent_id, lang, symbols, raw_edges);
            }
            "class_declaration" => {
                extract_js_class(source, &child, file_path, parent_id, lang, symbols, raw_edges);
            }
            "import_statement" => {
                extract_js_import(source, &child, file_path, parent_id, lang, symbols, raw_edges);
            }
            "export_statement" => {
                // Recurse into exported declarations
                walk_js(source, &child, file_path, parent_id, lang, symbols, raw_edges);
            }
            "lexical_declaration" => {
                // Handle: const foo = () => {} or const Foo = function() {}
                extract_js_variable_fn(source, &child, file_path, parent_id, lang, symbols, raw_edges);
            }
            "interface_declaration" | "type_alias_declaration" => {
                extract_js_type(source, &child, file_path, parent_id, lang, symbols);
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
                        doc_comment: None,
                        visibility: Visibility::Public,
                        language: lang,
                        parent: Some(parent_id),
                    });
                }
            }
            _ => {
                walk_js(source, &child, file_path, parent_id, lang, symbols, raw_edges);
            }
        }
    }
}

fn extract_js_function(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    lang: Language,
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

    let return_type = child_by_field(node, "return_type")
        .map(|r| format!(": {}", node_text(source, &r)))
        .unwrap_or_default();

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, SymbolKind::Function,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Function,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("function {name}{params}{return_type}")),
        doc_comment: None,
        visibility: Visibility::Public,
        language: lang,
        parent: Some(parent_id),
    });

    // Extract calls
    if let Some(body) = child_by_field(node, "body") {
        extract_js_calls(source, &body, id, None, raw_edges);
    }
}

fn extract_js_class(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    lang: Language,
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
        visibility: Visibility::Public,
        language: lang,
        parent: Some(parent_id),
    });

    // Check for extends
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "class_heritage" {
            let heritage_line = child.start_position().row as u32;
            let heritage_text = node_text(source, &child);
            if let Some(base) = heritage_text.strip_prefix("extends ") {
                let base = base.split_whitespace().next().unwrap_or(base);
                raw_edges.push(RawEdge {
                    source: id,
                    kind: EdgeKind::Extends,
                    target_name: base.to_string(),
                    target_module: None,
                    source_line: heritage_line,
                });
            }
            if heritage_text.contains("implements ") {
                if let Some(impl_part) = heritage_text.split("implements ").nth(1) {
                    for iface in impl_part.split(',') {
                        raw_edges.push(RawEdge {
                            source: id,
                            kind: EdgeKind::Implements,
                            target_name: iface.trim().to_string(),
                            target_module: None,
                            source_line: heritage_line,
                        });
                    }
                }
            }
        }
    }

    // Extract methods in class body
    if let Some(body) = child_by_field(node, "body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "method_definition" || child.kind() == "public_field_definition" {
                if let Some(name_node) = child_by_field(&child, "name") {
                    let method_name = node_text(source, &name_node);
                    let kind = if method_name == "constructor" {
                        SymbolKind::Constructor
                    } else if child.kind() == "public_field_definition" {
                        SymbolKind::Property
                    } else {
                        SymbolKind::Method
                    };

                    let method_id = NodeId::new(
                        &file_path.to_string_lossy(),
                        &format!("{name}.{method_name}"),
                        kind,
                        child.start_position().row as u32,
                    );

                    let params = child_by_field(&child, "parameters")
                        .map(|p| node_text(source, &p).to_string())
                        .unwrap_or_default();

                    symbols.push(SymbolNode {
                        id: method_id,
                        name: format!("{name}.{method_name}"),
                        kind,
                        file_path: file_path.to_path_buf(),
                        line_range: (child.start_position().row as u32, child.end_position().row as u32),
                        signature: Some(format!("{method_name}{params}")),
                        doc_comment: None,
                        visibility: Visibility::Public,
                        language: lang,
                        parent: Some(id),
                    });

                    if let Some(body) = child_by_field(&child, "body") {
                        extract_js_calls(source, &body, method_id, Some(name), raw_edges);
                    }
                }
            }
        }
    }
}

fn extract_js_import(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    lang: Language,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let text = node_text(source, node).trim().to_string();

    // Extract the source module from import statement
    let source_module = child_by_field(node, "source")
        .map(|s| {
            let t = node_text(source, &s);
            t.trim_matches(|c| c == '\'' || c == '"').to_string()
        });

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
        language: lang,
        parent: Some(parent_id),
    });

    raw_edges.push(RawEdge {
        source: parent_id,
        kind: EdgeKind::Imports,
        target_name: source_module.unwrap_or(text),
        target_module: None,
        source_line: node.start_position().row as u32,
    });
}

fn extract_js_variable_fn(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    lang: Language,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            let name_node = match child_by_field(&child, "name") {
                Some(n) => n,
                None => continue,
            };
            let value = match child_by_field(&child, "value") {
                Some(v) => v,
                None => continue,
            };

            // Only extract if the value is a function expression or arrow function
            if value.kind() == "arrow_function" || value.kind() == "function" {
                let name = node_text(source, &name_node);
                let params = child_by_field(&value, "parameters")
                    .map(|p| node_text(source, &p).to_string())
                    .unwrap_or_else(|| "()".to_string());

                let id = NodeId::new(
                    &file_path.to_string_lossy(), name, SymbolKind::Function,
                    child.start_position().row as u32,
                );

                symbols.push(SymbolNode {
                    id,
                    name: name.to_string(),
                    kind: SymbolKind::Function,
                    file_path: file_path.to_path_buf(),
                    line_range: (node.start_position().row as u32, node.end_position().row as u32),
                    signature: Some(format!("const {name} = {params} =>")),
                    doc_comment: None,
                    visibility: Visibility::Public,
                    language: lang,
                    parent: Some(parent_id),
                });

                if let Some(body) = child_by_field(&value, "body") {
                    extract_js_calls(source, &body, id, None, raw_edges);
                }
            }
        }
    }
}

fn extract_js_type(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    lang: Language,
    symbols: &mut Vec<SymbolNode>,
) {
    let name_node = match child_by_field(node, "name") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let kind = if node.kind() == "interface_declaration" {
        SymbolKind::Interface
    } else {
        SymbolKind::TypeAlias
    };

    let id = NodeId::new(
        &file_path.to_string_lossy(), name, kind,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!(
            "{} {name}",
            if kind == SymbolKind::Interface { "interface" } else { "type" }
        )),
        doc_comment: None,
        visibility: Visibility::Public,
        language: lang,
        parent: Some(parent_id),
    });
}

fn extract_js_calls(
    source: &[u8],
    node: &tree_sitter::Node,
    parent_id: NodeId,
    class_name: Option<&str>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
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
        extract_js_calls(source, &child, parent_id, class_name, raw_edges);
    }
}
