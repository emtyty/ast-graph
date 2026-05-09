//! PHP extractor — classes, interfaces, traits, enums (PHP 8+), functions,
//! methods, constructors, namespaces, `use` imports, properties, constants.
//!
//! Tree-sitter PHP node kinds: `namespace_definition`, `namespace_use_declaration`,
//! `class_declaration`, `interface_declaration`, `trait_declaration`,
//! `enum_declaration`, `function_definition`, `method_declaration`,
//! `property_declaration`, `const_declaration`.

use ast_graph_core::*;
use crate::extractor::*;
use std::path::Path;

pub struct PhpExtractor;

impl LanguageExtractor for PhpExtractor {
    fn language(&self) -> Language {
        Language::Php
    }

    fn file_extensions(&self) -> &[&str] {
        &["php", "phtml"]
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_php::LANGUAGE_PHP.into()
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
            language: Language::Php,
            parent: None,
        });

        walk_php(
            source,
            &tree.root_node(),
            file_path,
            file_node_id,
            None,
            &mut symbols,
            &mut raw_edges,
        );
        ExtractResult { symbols, raw_edges }
    }
}

fn walk_php(
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
            "namespace_definition" => {
                extract_php_namespace(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "namespace_use_declaration" => {
                extract_php_use(source, &child, parent_id, raw_edges);
            }
            "class_declaration" => {
                extract_php_class(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "interface_declaration" => {
                extract_php_interface(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "trait_declaration" => {
                extract_php_trait(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "enum_declaration" => {
                extract_php_enum(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "function_definition" => {
                extract_php_function(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "method_declaration" => {
                extract_php_method(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
            "property_declaration" => {
                extract_php_property(source, &child, file_path, parent_id, class_name, symbols);
            }
            "const_declaration" => {
                extract_php_const(source, &child, file_path, parent_id, class_name, symbols);
            }
            _ => {
                walk_php(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
        }
    }
}

fn extract_php_namespace(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let name_node =
        child_by_field(node, "name").or_else(|| find_child_by_kind(node, "namespace_name"));
    let name = match name_node {
        Some(n) => node_text(source, &n).to_string(),
        None => return,
    };
    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &name,
        SymbolKind::Namespace,
        node.start_position().row as u32,
    );
    symbols.push(SymbolNode {
        id,
        name: name.clone(),
        kind: SymbolKind::Namespace,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("namespace {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: Visibility::Public,
        language: Language::Php,
        parent: Some(parent_id),
    });

    // Recurse into the namespace body (or sibling declarations for
    // file-scoped namespaces).
    let body = child_by_field(node, "body").or_else(|| {
        find_child_by_any_kind(node, &["compound_statement", "declaration_list"])
    });
    if let Some(b) = body {
        walk_php(source, &b, file_path, id, None, symbols, raw_edges);
    } else {
        walk_php(source, node, file_path, id, None, symbols, raw_edges);
    }
}

fn extract_php_use(
    source: &[u8],
    node: &tree_sitter::Node,
    parent_id: NodeId,
    raw_edges: &mut Vec<RawEdge>,
) {
    // `use Foo\Bar;` or `use Foo\Bar as Baz;`
    let text = node_text(source, node).trim().trim_end_matches(';').to_string();
    let cleaned = text.trim_start_matches("use ").trim().to_string();
    if cleaned.is_empty() {
        return;
    }
    raw_edges.push(RawEdge {
        source: parent_id,
        kind: EdgeKind::Imports,
        target_name: cleaned,
        target_module: None,
        source_line: node.start_position().row as u32,
    });
}

fn extract_php_class(
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
    let name = node_text(source, &name_node).to_string();
    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &name,
        SymbolKind::Class,
        node.start_position().row as u32,
    );
    symbols.push(SymbolNode {
        id,
        name: name.clone(),
        kind: SymbolKind::Class,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("class {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: Visibility::Public,
        language: Language::Php,
        parent: Some(parent_id),
    });

    // extends + implements
    if let Some(base_clause) =
        child_by_field(node, "base_clause").or_else(|| find_child_by_kind(node, "base_clause"))
    {
        let mut c = base_clause.walk();
        for n in base_clause.children(&mut c) {
            if matches!(n.kind(), "name" | "qualified_name") {
                raw_edges.push(RawEdge {
                    source: id,
                    kind: EdgeKind::Extends,
                    target_name: node_text(source, &n).to_string(),
                    target_module: None,
                    source_line: n.start_position().row as u32,
                });
            }
        }
    }
    if let Some(impl_clause) = find_child_by_kind(node, "class_interface_clause") {
        let mut c = impl_clause.walk();
        for n in impl_clause.children(&mut c) {
            if matches!(n.kind(), "name" | "qualified_name") {
                raw_edges.push(RawEdge {
                    source: id,
                    kind: EdgeKind::Implements,
                    target_name: node_text(source, &n).to_string(),
                    target_module: None,
                    source_line: n.start_position().row as u32,
                });
            }
        }
    }

    // Body
    if let Some(body) = child_by_field(node, "body")
        .or_else(|| find_child_by_any_kind(node, &["declaration_list", "class_body"]))
    {
        walk_php(source, &body, file_path, id, Some(&name), symbols, raw_edges);
    }
}

fn extract_php_interface(
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
    let name = node_text(source, &name_node).to_string();
    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &name,
        SymbolKind::Interface,
        node.start_position().row as u32,
    );
    symbols.push(SymbolNode {
        id,
        name: name.clone(),
        kind: SymbolKind::Interface,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("interface {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: Visibility::Public,
        language: Language::Php,
        parent: Some(parent_id),
    });
    if let Some(body) = child_by_field(node, "body")
        .or_else(|| find_child_by_kind(node, "declaration_list"))
    {
        walk_php(source, &body, file_path, id, Some(&name), symbols, raw_edges);
    }
}

fn extract_php_trait(
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
    let name = node_text(source, &name_node).to_string();
    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &name,
        SymbolKind::Trait,
        node.start_position().row as u32,
    );
    symbols.push(SymbolNode {
        id,
        name: name.clone(),
        kind: SymbolKind::Trait,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("trait {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: Visibility::Public,
        language: Language::Php,
        parent: Some(parent_id),
    });
    if let Some(body) = child_by_field(node, "body")
        .or_else(|| find_child_by_kind(node, "declaration_list"))
    {
        walk_php(source, &body, file_path, id, Some(&name), symbols, raw_edges);
    }
}

fn extract_php_enum(
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
    let name = node_text(source, &name_node).to_string();
    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &name,
        SymbolKind::Enum,
        node.start_position().row as u32,
    );
    symbols.push(SymbolNode {
        id,
        name: name.clone(),
        kind: SymbolKind::Enum,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("enum {name}")),
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: Visibility::Public,
        language: Language::Php,
        parent: Some(parent_id),
    });
    if let Some(body) = child_by_field(node, "body")
        .or_else(|| find_child_by_any_kind(node, &["enum_declaration_list", "declaration_list"]))
    {
        walk_php(source, &body, file_path, id, Some(&name), symbols, raw_edges);
    }
}

fn extract_php_function(
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
    let name = node_text(source, &name_node).to_string();
    let params_text = child_by_field(node, "parameters")
        .map(|p| node_text(source, &p).to_string())
        .unwrap_or_else(|| "()".to_string());
    let signature = Some(format!("function {name}{params_text}"));
    let id = NodeId::new_with_sig(
        &file_path.to_string_lossy(),
        &name,
        SymbolKind::Function,
        node.start_position().row as u32,
        signature.as_deref(),
    );
    symbols.push(SymbolNode {
        id,
        name: name.clone(),
        kind: SymbolKind::Function,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature,
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: Visibility::Public,
        language: Language::Php,
        parent: Some(parent_id),
    });
    if let Some(body) = child_by_field(node, "body") {
        extract_php_calls(source, &body, id, None, raw_edges);
    }
}

fn extract_php_method(
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
    let bare = node_text(source, &name_node).to_string();
    let qualified = match class_name {
        Some(cn) => format!("{cn}.{bare}"),
        None => bare.clone(),
    };
    let params_text = child_by_field(node, "parameters")
        .map(|p| node_text(source, &p).to_string())
        .unwrap_or_else(|| "()".to_string());
    let kind = if bare == "__construct" {
        SymbolKind::Constructor
    } else {
        SymbolKind::Method
    };
    let signature = Some(format!("function {bare}{params_text}"));
    let id = NodeId::new_with_sig(
        &file_path.to_string_lossy(),
        &qualified,
        kind,
        node.start_position().row as u32,
        signature.as_deref(),
    );
    symbols.push(SymbolNode {
        id,
        name: qualified,
        kind,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature,
        doc_comment: extract_preceding_doc_comment(source, node),
        visibility: extract_php_visibility(node, source),
        language: Language::Php,
        parent: Some(parent_id),
    });
    if let Some(body) = child_by_field(node, "body") {
        extract_php_calls(source, &body, id, class_name, raw_edges);
    }
}

fn extract_php_property(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    class_name: Option<&str>,
    symbols: &mut Vec<SymbolNode>,
) {
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.kind() == "property_element" {
            // First child is `variable_name` containing `$name`.
            let mut c2 = c.walk();
            for v in c.children(&mut c2) {
                if v.kind() == "variable_name" {
                    let raw = node_text(source, &v).trim_start_matches('$').to_string();
                    if raw.is_empty() {
                        continue;
                    }
                    let qualified = match class_name {
                        Some(cn) => format!("{cn}.{raw}"),
                        None => raw.clone(),
                    };
                    let id = NodeId::new(
                        &file_path.to_string_lossy(),
                        &qualified,
                        SymbolKind::Property,
                        c.start_position().row as u32,
                    );
                    symbols.push(SymbolNode {
                        id,
                        name: qualified,
                        kind: SymbolKind::Property,
                        file_path: file_path.to_path_buf(),
                        line_range: (c.start_position().row as u32, c.end_position().row as u32),
                        signature: None,
                        doc_comment: extract_preceding_doc_comment(source, node),
                        visibility: extract_php_visibility(node, source),
                        language: Language::Php,
                        parent: Some(parent_id),
                    });
                }
            }
        }
    }
}

fn extract_php_const(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    class_name: Option<&str>,
    symbols: &mut Vec<SymbolNode>,
) {
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if c.kind() == "const_element" {
            let mut c2 = c.walk();
            for v in c.children(&mut c2) {
                if v.kind() == "name" {
                    let raw = node_text(source, &v).to_string();
                    let qualified = match class_name {
                        Some(cn) => format!("{cn}.{raw}"),
                        None => raw.clone(),
                    };
                    let id = NodeId::new(
                        &file_path.to_string_lossy(),
                        &qualified,
                        SymbolKind::Constant,
                        c.start_position().row as u32,
                    );
                    symbols.push(SymbolNode {
                        id,
                        name: qualified,
                        kind: SymbolKind::Constant,
                        file_path: file_path.to_path_buf(),
                        line_range: (c.start_position().row as u32, c.end_position().row as u32),
                        signature: None,
                        doc_comment: extract_preceding_doc_comment(source, node),
                        visibility: extract_php_visibility(node, source),
                        language: Language::Php,
                        parent: Some(parent_id),
                    });
                    break;
                }
            }
        }
    }
}

fn extract_php_calls(
    source: &[u8],
    node: &tree_sitter::Node,
    parent_id: NodeId,
    class_name: Option<&str>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "function_call_expression" => {
                if let Some(func) = child_by_field(&child, "function") {
                    let raw = node_text(source, &func).to_string();
                    let target = qualify_php_call(&raw, class_name);
                    if !target.is_empty() {
                        raw_edges.push(RawEdge {
                            source: parent_id,
                            kind: EdgeKind::Calls,
                            target_name: target,
                            target_module: None,
                            source_line: child.start_position().row as u32,
                        });
                    }
                }
            }
            "member_call_expression" => {
                // `$this->foo()` or `$obj->foo()` — text is `$x->foo`.
                let object_node = child_by_field(&child, "object");
                let name_node = child_by_field(&child, "name");
                if let (Some(obj), Some(nm)) = (object_node, name_node) {
                    let obj_txt = node_text(source, &obj);
                    let nm_txt = node_text(source, &nm);
                    let target = if obj_txt == "$this" {
                        match class_name {
                            Some(cn) => format!("{cn}.{nm_txt}"),
                            None => nm_txt.to_string(),
                        }
                    } else {
                        format!("{obj_txt}.{nm_txt}")
                    };
                    raw_edges.push(RawEdge {
                        source: parent_id,
                        kind: EdgeKind::Calls,
                        target_name: target,
                        target_module: None,
                        source_line: child.start_position().row as u32,
                    });
                }
            }
            "scoped_call_expression" => {
                // `Foo::bar()` or `self::bar()` / `parent::bar()` / `static::bar()`.
                let scope_node = child_by_field(&child, "scope");
                let name_node = child_by_field(&child, "name");
                if let (Some(sc), Some(nm)) = (scope_node, name_node) {
                    let sc_txt = node_text(source, &sc);
                    let nm_txt = node_text(source, &nm);
                    let scope_name = match sc_txt {
                        "self" | "static" => class_name.unwrap_or("self").to_string(),
                        "parent" => "parent".to_string(),
                        s => s.to_string(),
                    };
                    raw_edges.push(RawEdge {
                        source: parent_id,
                        kind: EdgeKind::Calls,
                        target_name: format!("{scope_name}.{nm_txt}"),
                        target_module: None,
                        source_line: child.start_position().row as u32,
                    });
                }
            }
            "object_creation_expression" => {
                // `new Foo(...)` — emit a CALLS edge to Foo.__construct.
                let mut c2 = child.walk();
                for n in child.children(&mut c2) {
                    if matches!(n.kind(), "name" | "qualified_name") {
                        let target = format!("{}.__construct", node_text(source, &n));
                        raw_edges.push(RawEdge {
                            source: parent_id,
                            kind: EdgeKind::Calls,
                            target_name: target,
                            target_module: None,
                            source_line: child.start_position().row as u32,
                        });
                        break;
                    }
                }
            }
            _ => {}
        }
        extract_php_calls(source, &child, parent_id, class_name, raw_edges);
    }
}

/// Qualify a free-function-style PHP call relative to the enclosing class.
/// Most function calls in PHP are bare names; method calls are dispatched via
/// the dedicated `member_call_expression` / `scoped_call_expression` nodes,
/// not this branch.
fn qualify_php_call(raw: &str, _class_name: Option<&str>) -> String {
    // Strip leading namespace separator.
    raw.trim_start_matches('\\').to_string()
}

fn extract_php_visibility(node: &tree_sitter::Node, source: &[u8]) -> Visibility {
    let mut cursor = node.walk();
    for c in node.children(&mut cursor) {
        if matches!(c.kind(), "visibility_modifier" | "modifier") {
            let txt = node_text(source, &c);
            if txt.contains("private") {
                return Visibility::Private;
            }
            if txt.contains("protected") {
                return Visibility::Protected;
            }
            if txt.contains("public") {
                return Visibility::Public;
            }
        }
    }
    Visibility::Public
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> ExtractResult {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_php::LANGUAGE_PHP.into())
            .unwrap();
        let tree = parser.parse(source.as_bytes(), None).unwrap();
        PhpExtractor.extract(source.as_bytes(), &tree, Path::new("test.php"))
    }

    #[test]
    fn extracts_class_with_method() {
        let r = parse(
            "<?php\nclass Greeter {\n    public function hello() { return 'hi'; }\n}\n",
        );
        assert!(r
            .symbols
            .iter()
            .any(|s| s.name == "Greeter" && s.kind == SymbolKind::Class));
        assert!(r
            .symbols
            .iter()
            .any(|s| s.name == "Greeter.hello" && s.kind == SymbolKind::Method));
    }

    #[test]
    fn extracts_interface() {
        let r = parse("<?php\ninterface Animal { public function speak(); }\n");
        assert!(r
            .symbols
            .iter()
            .any(|s| s.name == "Animal" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn extracts_trait() {
        let r = parse("<?php\ntrait Loggable { public function log() {} }\n");
        assert!(r
            .symbols
            .iter()
            .any(|s| s.name == "Loggable" && s.kind == SymbolKind::Trait));
    }

    #[test]
    fn extracts_extends_and_implements() {
        let r = parse(
            "<?php\ninterface Animal {}\nclass Base {}\nclass Dog extends Base implements Animal {}\n",
        );
        assert!(r
            .raw_edges
            .iter()
            .any(|e| e.kind == EdgeKind::Extends && e.target_name == "Base"));
        assert!(r
            .raw_edges
            .iter()
            .any(|e| e.kind == EdgeKind::Implements && e.target_name == "Animal"));
    }

    #[test]
    fn this_call_qualifies_to_class() {
        let r = parse(
            "<?php\nclass C {\n  public function a() { $this->b(); }\n  public function b() {}\n}\n",
        );
        assert!(r
            .raw_edges
            .iter()
            .any(|e| e.kind == EdgeKind::Calls && e.target_name == "C.b"));
    }

    #[test]
    fn use_emits_imports_edge() {
        let r = parse("<?php\nuse App\\Http\\Controllers\\UserController;\n");
        assert!(r.raw_edges.iter().any(|e| e.kind == EdgeKind::Imports));
    }
}
