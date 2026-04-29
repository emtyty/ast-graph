use ast_graph_core::*;
use crate::extractor::*;
use std::path::Path;

pub struct GoExtractor;

impl LanguageExtractor for GoExtractor {
    fn language(&self) -> Language {
        Language::Go
    }

    fn file_extensions(&self) -> &[&str] {
        &["go"]
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_go::LANGUAGE.into()
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
            language: Language::Go,
            parent: None,
        });

        walk_go(source, &tree.root_node(), file_path, file_node_id, &mut symbols, &mut raw_edges);
        ExtractResult { symbols, raw_edges }
    }
}

fn walk_go(
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
            "package_clause" => {
                extract_go_package(source, &child, file_path, parent_id, symbols);
            }
            "import_declaration" => {
                extract_go_imports(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "function_declaration" => {
                extract_go_function(source, &child, file_path, parent_id, None, symbols, raw_edges);
            }
            "method_declaration" => {
                extract_go_method(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "type_declaration" => {
                extract_go_type_decl(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "const_declaration" => {
                extract_go_const(source, &child, file_path, parent_id, symbols);
            }
            _ => {}
        }
    }
}

// In Go, exported names start with an uppercase letter.
fn go_visibility(name: &str) -> Visibility {
    match name.chars().next() {
        Some(c) if c.is_uppercase() => Visibility::Public,
        _ => Visibility::Private,
    }
}

fn extract_go_package(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
) {
    let name_node = match find_child_by_kind(node, "package_identifier") {
        Some(n) => n,
        None => return,
    };
    let name = node_text(source, &name_node);

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        name,
        SymbolKind::Module,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Module,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("package {name}")),
        doc_comment: None,
        visibility: Visibility::Public,
        language: Language::Go,
        parent: Some(parent_id),
    });
}

fn extract_go_imports(
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
            "import_spec" => {
                extract_go_import_spec(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            // Grouped imports: `import ( "a" "b" )` wraps specs in import_spec_list.
            "import_spec_list" => {
                let mut c = child.walk();
                for spec in child.children(&mut c) {
                    if spec.kind() == "import_spec" {
                        extract_go_import_spec(source, &spec, file_path, parent_id, symbols, raw_edges);
                    }
                }
            }
            _ => {}
        }
    }
}

fn extract_go_import_spec(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let path_node = match child_by_field(node, "path") {
        Some(n) => n,
        None => return,
    };
    // Strip surrounding quotes from the string literal.
    let raw = node_text(source, &path_node);
    let import_path = raw.trim_matches('"').trim_matches('`');

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        import_path,
        SymbolKind::Import,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: import_path.to_string(),
        kind: SymbolKind::Import,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("import \"{import_path}\"")),
        doc_comment: None,
        visibility: Visibility::Public,
        language: Language::Go,
        parent: Some(parent_id),
    });

    raw_edges.push(RawEdge {
        source: parent_id,
        kind: EdgeKind::Imports,
        target_name: import_path.to_string(),
        target_module: None,
        source_line: node.start_position().row as u32,
    });
}

/// Extract receiver type name from a method_declaration node, stripping any pointer `*`.
fn extract_receiver_type(source: &[u8], node: &tree_sitter::Node) -> Option<String> {
    let receiver = child_by_field(node, "receiver")?;
    // receiver is a parameter_list; find the single parameter_declaration inside it.
    let mut cursor = receiver.walk();
    for child in receiver.children(&mut cursor) {
        if child.kind() == "parameter_declaration" {
            let type_node = child_by_field(&child, "type")?;
            let type_text = node_text(source, &type_node);
            // Strip pointer marker and any parentheses from generic receivers.
            let clean = type_text
                .trim_start_matches('*')
                .trim_start_matches('(')
                .trim_end_matches(')')
                .split('[') // strip type parameters: T[K] → T
                .next()
                .unwrap_or(type_text)
                .trim();
            return Some(clean.to_string());
        }
    }
    None
}

fn extract_go_method(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let receiver_type = extract_receiver_type(source, node);
    extract_go_function(source, node, file_path, parent_id, receiver_type.as_deref(), symbols, raw_edges);
}

fn extract_go_function(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    receiver_type: Option<&str>,
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

    let result = child_by_field(node, "result")
        .map(|r| format!(" {}", node_text(source, &r)))
        .unwrap_or_default();

    let (kind, qualified_name) = match receiver_type {
        Some(recv) => (SymbolKind::Method, format!("{recv}.{name}")),
        None => (SymbolKind::Function, name.to_string()),
    };

    let visibility = go_visibility(name);

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &qualified_name,
        kind,
        node.start_position().row as u32,
    );

    let receiver_sig = match receiver_type {
        Some(recv) => format!("({recv}) "),
        None => String::new(),
    };

    symbols.push(SymbolNode {
        id,
        name: qualified_name.clone(),
        kind,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("func {receiver_sig}{name}{params}{result}")),
        doc_comment: None,
        visibility,
        language: Language::Go,
        parent: Some(parent_id),
    });

    if let Some(body) = child_by_field(node, "body") {
        extract_go_calls(source, &body, id, receiver_type, raw_edges);
    }
}

fn extract_go_type_decl(
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
            "type_spec" => {
                extract_go_type_spec(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "type_alias" => {
                extract_go_type_alias(source, &child, file_path, parent_id, symbols);
            }
            _ => {}
        }
    }
}

fn extract_go_type_spec(
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

    let type_node = match child_by_field(node, "type") {
        Some(t) => t,
        None => return,
    };

    let (kind, sig) = match type_node.kind() {
        "struct_type" => (SymbolKind::Struct, format!("type {name} struct")),
        "interface_type" => (SymbolKind::Interface, format!("type {name} interface")),
        _ => (
            SymbolKind::TypeAlias,
            format!("type {name} {}", node_text(source, &type_node)),
        ),
    };

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        name,
        kind,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(sig),
        doc_comment: None,
        visibility: go_visibility(name),
        language: Language::Go,
        parent: Some(parent_id),
    });

    match type_node.kind() {
        "struct_type" => extract_go_struct_fields(source, &type_node, file_path, id, name, symbols, raw_edges),
        "interface_type" => extract_go_interface_methods(source, &type_node, file_path, id, name, symbols),
        _ => {}
    }
}

fn extract_go_type_alias(
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

    let type_text = child_by_field(node, "type")
        .map(|t| node_text(source, &t).to_string())
        .unwrap_or_default();

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        name,
        SymbolKind::TypeAlias,
        node.start_position().row as u32,
    );

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::TypeAlias,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("type {name} = {type_text}")),
        doc_comment: None,
        visibility: go_visibility(name),
        language: Language::Go,
        parent: Some(parent_id),
    });
}

fn extract_go_struct_fields(
    source: &[u8],
    struct_node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    struct_name: &str,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    // struct_type → field_declaration_list → field_declaration*
    let list = match find_child_by_kind(struct_node, "field_declaration_list") {
        Some(n) => n,
        None => return,
    };

    let mut cursor = list.walk();
    for child in list.children(&mut cursor) {
        if child.kind() != "field_declaration" {
            continue;
        }

        // Collect all field_identifier children (a single declaration can name multiple fields).
        let mut field_cursor = child.walk();
        let field_names: Vec<_> = child
            .children(&mut field_cursor)
            .filter(|n| n.kind() == "field_identifier")
            .map(|n| node_text(source, &n).to_string())
            .collect();

        // Emit a REFERENCES edge for the field type if present.
        let type_text = child_by_field(&child, "type")
            .map(|t| node_text(source, &t).to_string());

        for field_name in field_names {
            let qualified = format!("{struct_name}.{field_name}");
            let id = NodeId::new(
                &file_path.to_string_lossy(),
                &qualified,
                SymbolKind::Field,
                child.start_position().row as u32,
            );

            symbols.push(SymbolNode {
                id,
                name: qualified,
                kind: SymbolKind::Field,
                file_path: file_path.to_path_buf(),
                line_range: (child.start_position().row as u32, child.end_position().row as u32),
                signature: type_text.as_deref().map(|t| format!("{field_name} {t}")),
                doc_comment: None,
                visibility: go_visibility(&field_name),
                language: Language::Go,
                parent: Some(parent_id),
            });

            if let Some(ref type_name) = type_text {
                let bare = type_name
                    .trim_start_matches('*')
                    .trim_start_matches('[')
                    .trim();
                if !bare.is_empty() && bare.chars().next().map_or(false, |c| c.is_uppercase()) {
                    raw_edges.push(RawEdge {
                        source: id,
                        kind: EdgeKind::References,
                        target_name: bare.to_string(),
                        target_module: None,
                        source_line: child.start_position().row as u32,
                    });
                }
            }
        }
    }
}

fn extract_go_interface_methods(
    source: &[u8],
    iface_node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    iface_name: &str,
    symbols: &mut Vec<SymbolNode>,
) {
    let mut cursor = iface_node.walk();
    for child in iface_node.children(&mut cursor) {
        // tree-sitter-go 0.25 uses `method_elem`; older versions used `method_spec`.
        if child.kind() != "method_elem" && child.kind() != "method_spec" {
            continue;
        }

        // Name is either a named `name` field or the first `field_identifier` child.
        let name_node = match child_by_field(&child, "name")
            .or_else(|| find_child_by_kind(&child, "field_identifier"))
        {
            Some(n) => n,
            None => continue,
        };
        let name = node_text(source, &name_node);

        let params = child_by_field(&child, "parameters")
            .map(|p| node_text(source, &p).to_string())
            .unwrap_or_else(|| "()".to_string());

        let result = child_by_field(&child, "result")
            .map(|r| format!(" {}", node_text(source, &r)))
            .unwrap_or_default();

        let qualified = format!("{iface_name}.{name}");
        let id = NodeId::new(
            &file_path.to_string_lossy(),
            &qualified,
            SymbolKind::Method,
            child.start_position().row as u32,
        );

        symbols.push(SymbolNode {
            id,
            name: qualified,
            kind: SymbolKind::Method,
            file_path: file_path.to_path_buf(),
            line_range: (child.start_position().row as u32, child.end_position().row as u32),
            signature: Some(format!("{name}{params}{result}")),
            doc_comment: None,
            visibility: go_visibility(name),
            language: Language::Go,
            parent: Some(parent_id),
        });
    }
}

fn extract_go_const(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "const_spec" {
            continue;
        }

        // const_spec names live in an identifier_list or directly as identifiers.
        let name_node = match child_by_field(&child, "name") {
            Some(n) => n,
            None => {
                // Some grammars put names directly as identifier children.
                let mut c = child.walk();
                let first_ident = child.children(&mut c).find(|n| n.kind() == "identifier");
                match first_ident {
                    Some(n) => n,
                    None => continue,
                }
            }
        };

        let name = node_text(source, &name_node);
        let id = NodeId::new(
            &file_path.to_string_lossy(),
            name,
            SymbolKind::Constant,
            child.start_position().row as u32,
        );

        let value_text = child_by_field(&child, "value")
            .map(|v| format!(" = {}", node_text(source, &v)))
            .unwrap_or_default();

        symbols.push(SymbolNode {
            id,
            name: name.to_string(),
            kind: SymbolKind::Constant,
            file_path: file_path.to_path_buf(),
            line_range: (child.start_position().row as u32, child.end_position().row as u32),
            signature: Some(format!("const {name}{value_text}")),
            doc_comment: None,
            visibility: go_visibility(name),
            language: Language::Go,
            parent: Some(parent_id),
        });
    }
}

fn extract_go_calls(
    source: &[u8],
    node: &tree_sitter::Node,
    caller_id: NodeId,
    receiver_type: Option<&str>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call_expression" {
            if let Some(func) = child_by_field(&child, "function") {
                let raw_target = node_text(source, &func);
                // Go has no unambiguous `self`/`this` keyword, so we cannot safely
                // strip receiver prefixes without type information.  Emit the raw
                // selector text (e.g. "fmt.Sprintf", "g.Save") and let the resolver
                // do best-effort matching.  For bare identifiers ("StandaloneHelper")
                // resolution works directly.
                let call_target = match receiver_type {
                    Some(recv_type) => qualify_go_self_call(raw_target, recv_type),
                    None => raw_target.to_string(),
                };
                raw_edges.push(RawEdge {
                    source: caller_id,
                    kind: EdgeKind::Calls,
                    target_name: call_target,
                    target_module: None,
                    source_line: child.start_position().row as u32,
                });
            }
        }
        extract_go_calls(source, &child, caller_id, receiver_type, raw_edges);
    }
}

/// Qualify calls that use the receiver variable as the operand.
/// We only rewrite when the operand is a short single-letter or two-letter
/// lowercase identifier that matches common Go receiver conventions AND the
/// method part has no further dots (no chaining).  Everything else is emitted
/// as-is so package calls like `fmt.Sprintf` are preserved.
fn qualify_go_self_call(target: &str, recv_type: &str) -> String {
    let dot = match target.find('.') {
        Some(p) => p,
        None => return target.to_string(),
    };

    let operand = &target[..dot];
    let method = &target[dot + 1..];

    // Multi-level chains (a.b.c) — leave unchanged.
    if method.contains('.') {
        return target.to_string();
    }

    // Only rewrite very short lowercase identifiers (1-2 chars) which are the
    // idiomatic Go receiver variable convention.  Longer identifiers are more
    // likely to be package names or non-receiver variables.
    let is_likely_receiver = operand.len() <= 2
        && operand.chars().all(|c| c.is_lowercase());

    if is_likely_receiver {
        format!("{recv_type}.{method}")
    } else {
        target.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::LanguageExtractor;
    use std::path::Path;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Parse `src` as Go and return the extracted (symbols, raw_edges).
    fn extract(src: &str) -> (Vec<SymbolNode>, Vec<RawEdge>) {
        let extractor = GoExtractor;
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&extractor.tree_sitter_language())
            .expect("tree-sitter-go language load failed");
        let tree = parser.parse(src.as_bytes(), None).expect("parse failed");
        let result = extractor.extract(src.as_bytes(), &tree, Path::new("test.go"));
        (result.symbols, result.raw_edges)
    }

    fn find_symbol<'a>(symbols: &'a [SymbolNode], name: &str) -> Option<&'a SymbolNode> {
        symbols.iter().find(|s| s.name == name)
    }

    fn find_edges_from<'a>(edges: &'a [RawEdge], source_id: NodeId) -> Vec<&'a RawEdge> {
        edges.iter().filter(|e| e.source == source_id).collect()
    }

    fn edge_targets<'a>(edges: &[&'a RawEdge]) -> Vec<&'a str> {
        let mut v: Vec<&str> = edges.iter().map(|e| e.target_name.as_str()).collect();
        v.sort_unstable();
        v
    }

    // ── visibility ───────────────────────────────────────────────────────────

    #[test]
    fn visibility_uppercase_is_public() {
        assert_eq!(go_visibility("Exported"), Visibility::Public);
        assert_eq!(go_visibility("MAX"), Visibility::Public);
    }

    #[test]
    fn visibility_lowercase_is_private() {
        assert_eq!(go_visibility("unexported"), Visibility::Private);
        assert_eq!(go_visibility("count"), Visibility::Private);
    }

    #[test]
    fn visibility_empty_is_private() {
        assert_eq!(go_visibility(""), Visibility::Private);
    }

    // ── qualify_go_self_call ─────────────────────────────────────────────────

    #[test]
    fn qualify_short_receiver_var() {
        // "s" is a 1-char lowercase var → rewrite to Type.Method
        assert_eq!(qualify_go_self_call("s.Save", "Store"), "Store.Save");
        assert_eq!(qualify_go_self_call("g.Greet", "Greeter"), "Greeter.Greet");
    }

    #[test]
    fn qualify_two_char_receiver_var() {
        assert_eq!(qualify_go_self_call("uc.Do", "UseCase"), "UseCase.Do");
    }

    #[test]
    fn no_qualify_package_call() {
        // "fmt" is 3 chars → treated as a package, left as-is
        assert_eq!(qualify_go_self_call("fmt.Sprintf", "Any"), "fmt.Sprintf");
        assert_eq!(qualify_go_self_call("strings.ToUpper", "Any"), "strings.ToUpper");
    }

    #[test]
    fn no_qualify_bare_function() {
        assert_eq!(qualify_go_self_call("helper", "Any"), "helper");
    }

    #[test]
    fn no_qualify_chained_call() {
        assert_eq!(qualify_go_self_call("s.db.Query", "Store"), "s.db.Query");
    }

    // ── package ──────────────────────────────────────────────────────────────

    #[test]
    fn extracts_package_as_module() {
        let (symbols, _) = extract("package myapp\n");
        let pkg = find_symbol(&symbols, "myapp").expect("package symbol missing");
        assert_eq!(pkg.kind, SymbolKind::Module);
        assert_eq!(pkg.visibility, Visibility::Public);
        assert_eq!(pkg.language, Language::Go);
        assert_eq!(pkg.signature.as_deref(), Some("package myapp"));
    }

    // ── imports ──────────────────────────────────────────────────────────────

    #[test]
    fn extracts_single_import() {
        let src = "package p\nimport \"fmt\"\n";
        let (symbols, edges) = extract(src);

        let imp = find_symbol(&symbols, "fmt").expect("import symbol missing");
        assert_eq!(imp.kind, SymbolKind::Import);
        assert_eq!(imp.signature.as_deref(), Some("import \"fmt\""));

        let import_edge = edges.iter().find(|e| e.kind == EdgeKind::Imports && e.target_name == "fmt");
        assert!(import_edge.is_some(), "IMPORTS edge for fmt missing");
    }

    #[test]
    fn extracts_grouped_imports() {
        let src = "package p\nimport (\n\t\"fmt\"\n\t\"strings\"\n)\n";
        let (symbols, edges) = extract(src);

        assert!(find_symbol(&symbols, "fmt").is_some());
        assert!(find_symbol(&symbols, "strings").is_some());

        let import_edges: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Imports).collect();
        assert_eq!(import_edges.len(), 2);
    }

    // ── functions ────────────────────────────────────────────────────────────

    #[test]
    fn extracts_exported_function() {
        let src = "package p\nfunc Add(a, b int) int { return a + b }\n";
        let (symbols, _) = extract(src);

        let f = find_symbol(&symbols, "Add").expect("Add missing");
        assert_eq!(f.kind, SymbolKind::Function);
        assert_eq!(f.visibility, Visibility::Public);
        assert_eq!(f.language, Language::Go);
        assert!(f.signature.as_deref().unwrap_or("").contains("func Add"));
    }

    #[test]
    fn extracts_unexported_function() {
        let src = "package p\nfunc helper() {}\n";
        let (symbols, _) = extract(src);

        let f = find_symbol(&symbols, "helper").expect("helper missing");
        assert_eq!(f.kind, SymbolKind::Function);
        assert_eq!(f.visibility, Visibility::Private);
    }

    #[test]
    fn function_line_range_is_correct() {
        let src = "package p\n\nfunc Foo() {\n}\n";
        let (symbols, _) = extract(src);
        let f = find_symbol(&symbols, "Foo").expect("Foo missing");
        // line_start should be 2 (0-indexed row of "func Foo")
        assert_eq!(f.line_range.0, 2);
    }

    // ── methods ──────────────────────────────────────────────────────────────

    #[test]
    fn extracts_method_qualified_with_receiver_type() {
        let src = "package p\ntype Dog struct{}\nfunc (d *Dog) Bark() string { return \"woof\" }\n";
        let (symbols, _) = extract(src);

        let m = find_symbol(&symbols, "Dog.Bark").expect("Dog.Bark missing");
        assert_eq!(m.kind, SymbolKind::Method);
        assert_eq!(m.visibility, Visibility::Public);
        assert!(m.signature.as_deref().unwrap_or("").contains("func"));
        assert!(m.signature.as_deref().unwrap_or("").contains("Bark"));
    }

    #[test]
    fn extracts_unexported_method() {
        let src = "package p\ntype T struct{}\nfunc (t T) reset() {}\n";
        let (symbols, _) = extract(src);

        let m = find_symbol(&symbols, "T.reset").expect("T.reset missing");
        assert_eq!(m.kind, SymbolKind::Method);
        assert_eq!(m.visibility, Visibility::Private);
    }

    #[test]
    fn pointer_receiver_strips_star() {
        // *Dog receiver → method should be "Dog.Bark", not "*Dog.Bark"
        let src = "package p\ntype Dog struct{}\nfunc (d *Dog) Bark() {}\n";
        let (symbols, _) = extract(src);
        assert!(find_symbol(&symbols, "Dog.Bark").is_some());
        assert!(find_symbol(&symbols, "*Dog.Bark").is_none());
    }

    // ── structs & fields ─────────────────────────────────────────────────────

    #[test]
    fn extracts_struct() {
        let src = "package p\ntype Point struct {\n\tX float64\n\tY float64\n}\n";
        let (symbols, _) = extract(src);

        let s = find_symbol(&symbols, "Point").expect("Point missing");
        assert_eq!(s.kind, SymbolKind::Struct);
        assert_eq!(s.visibility, Visibility::Public);
        assert_eq!(s.signature.as_deref(), Some("type Point struct"));
    }

    #[test]
    fn extracts_struct_fields_with_visibility() {
        let src = "package p\ntype Account struct {\n\tID string\n\tbalance float64\n}\n";
        let (symbols, _) = extract(src);

        let pub_field = find_symbol(&symbols, "Account.ID").expect("Account.ID missing");
        assert_eq!(pub_field.kind, SymbolKind::Field);
        assert_eq!(pub_field.visibility, Visibility::Public);

        let priv_field = find_symbol(&symbols, "Account.balance").expect("Account.balance missing");
        assert_eq!(priv_field.kind, SymbolKind::Field);
        assert_eq!(priv_field.visibility, Visibility::Private);
    }

    #[test]
    fn struct_field_emits_references_edge_for_exported_type() {
        let src = "package p\ntype Order struct {\n\tUser User\n\tcount int\n}\n";
        let (symbols, edges) = extract(src);

        let field = find_symbol(&symbols, "Order.User").expect("Order.User missing");
        let refs: Vec<_> = edges
            .iter()
            .filter(|e| e.source == field.id && e.kind == EdgeKind::References)
            .collect();
        assert!(!refs.is_empty(), "REFERENCES edge from Order.User missing");
        assert_eq!(refs[0].target_name, "User");

        // unexported type "int" — no REFERENCES edge
        let int_field = find_symbol(&symbols, "Order.count").expect("Order.count missing");
        let int_refs: Vec<_> = edges
            .iter()
            .filter(|e| e.source == int_field.id && e.kind == EdgeKind::References)
            .collect();
        assert!(int_refs.is_empty(), "unexpected REFERENCES edge for builtin type");
    }

    // ── interfaces ───────────────────────────────────────────────────────────

    #[test]
    fn extracts_interface() {
        let src = "package p\ntype Writer interface {\n\tWrite(p []byte) (int, error)\n\tClose() error\n}\n";
        let (symbols, _) = extract(src);

        let iface = find_symbol(&symbols, "Writer").expect("Writer missing");
        assert_eq!(iface.kind, SymbolKind::Interface);
        assert_eq!(iface.signature.as_deref(), Some("type Writer interface"));

        let write_m = find_symbol(&symbols, "Writer.Write").expect("Writer.Write missing");
        assert_eq!(write_m.kind, SymbolKind::Method);

        let close_m = find_symbol(&symbols, "Writer.Close").expect("Writer.Close missing");
        assert_eq!(close_m.kind, SymbolKind::Method);
    }

    // ── type aliases ─────────────────────────────────────────────────────────

    #[test]
    fn extracts_type_alias() {
        let src = "package p\ntype MyInt = int\n";
        let (symbols, _) = extract(src);
        let t = find_symbol(&symbols, "MyInt").expect("MyInt missing");
        assert_eq!(t.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn extracts_named_type() {
        let src = "package p\ntype Duration int64\n";
        let (symbols, _) = extract(src);
        let t = find_symbol(&symbols, "Duration").expect("Duration missing");
        assert_eq!(t.kind, SymbolKind::TypeAlias);
    }

    // ── constants ────────────────────────────────────────────────────────────

    #[test]
    fn extracts_constants() {
        let src = "package p\nconst MaxRetries = 3\nconst defaultTimeout = 30\n";
        let (symbols, _) = extract(src);

        let pub_c = find_symbol(&symbols, "MaxRetries").expect("MaxRetries missing");
        assert_eq!(pub_c.kind, SymbolKind::Constant);
        assert_eq!(pub_c.visibility, Visibility::Public);

        let priv_c = find_symbol(&symbols, "defaultTimeout").expect("defaultTimeout missing");
        assert_eq!(priv_c.kind, SymbolKind::Constant);
        assert_eq!(priv_c.visibility, Visibility::Private);
    }

    // ── call edges ───────────────────────────────────────────────────────────

    #[test]
    fn emits_calls_edge_for_bare_function() {
        let src = "package p\nfunc helper() {}\nfunc Run() { helper() }\n";
        let (symbols, edges) = extract(src);

        let run = find_symbol(&symbols, "Run").expect("Run missing");
        let calls = find_edges_from(&edges, run.id);
        let targets = edge_targets(&calls);
        assert!(targets.contains(&"helper"), "expected CALLS helper, got: {:?}", targets);
    }

    #[test]
    fn emits_calls_edge_qualified_for_receiver_var() {
        // "s" is a 1-char receiver var → "s.Save" should become "Store.Save"
        let src = "package p\ntype Store struct{}\nfunc (s *Store) Commit() { s.Save() }\nfunc (s *Store) Save() {}\n";
        let (symbols, edges) = extract(src);

        let commit = find_symbol(&symbols, "Store.Commit").expect("Store.Commit missing");
        let calls = find_edges_from(&edges, commit.id);
        let targets = edge_targets(&calls);
        assert!(
            targets.contains(&"Store.Save"),
            "expected CALLS Store.Save, got: {:?}",
            targets
        );
    }

    #[test]
    fn preserves_package_qualified_calls() {
        // "fmt" is 3 chars → left as fmt.Sprintf
        let src = "package p\nimport \"fmt\"\nfunc Log(msg string) { fmt.Println(msg) }\n";
        let (symbols, edges) = extract(src);

        let log = find_symbol(&symbols, "Log").expect("Log missing");
        let calls = find_edges_from(&edges, log.id);
        let targets = edge_targets(&calls);
        assert!(
            targets.contains(&"fmt.Println"),
            "expected CALLS fmt.Println, got: {:?}",
            targets
        );
    }

    // ── full file ────────────────────────────────────────────────────────────

    #[test]
    fn full_file_symbol_count_and_kinds() {
        let src = r#"package store

import (
    "fmt"
    "errors"
)

type ErrNotFound struct{ ID string }

type Repository interface {
    Find(id string) (*ErrNotFound, error)
    Save(v interface{}) error
}

type UserRepo struct {
    db     interface{}
    Logger *fmt.Stringer
}

func NewUserRepo(db interface{}) *UserRepo {
    return &UserRepo{db: db}
}

func (r *UserRepo) Find(id string) (*ErrNotFound, error) {
    if id == "" {
        return nil, errors.New("empty id")
    }
    return nil, nil
}

func (r *UserRepo) Save(v interface{}) error {
    return nil
}

const MaxResults = 100
"#;
        let (symbols, edges) = extract(src);

        // file + package + 2 imports + struct + interface + 2 iface methods
        // + struct + 2 fields + 1 function + 2 methods + 1 constant = 14 total
        let kinds: Vec<_> = symbols.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SymbolKind::File));
        assert!(kinds.contains(&SymbolKind::Module));
        assert!(kinds.contains(&SymbolKind::Import));
        assert!(kinds.contains(&SymbolKind::Struct));
        assert!(kinds.contains(&SymbolKind::Interface));
        assert!(kinds.contains(&SymbolKind::Method));
        assert!(kinds.contains(&SymbolKind::Function));
        assert!(kinds.contains(&SymbolKind::Constant));
        assert!(kinds.contains(&SymbolKind::Field));

        // at least 2 IMPORTS edges (fmt + errors)
        let import_edges: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Imports).collect();
        assert!(import_edges.len() >= 2);

        // CALLS edges present (Find calls errors.New)
        let call_edges: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Calls).collect();
        assert!(!call_edges.is_empty());
    }
}
