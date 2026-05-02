use ast_graph_core::*;
use crate::extractor::*;
use std::path::Path;

pub struct PythonExtractor;

impl LanguageExtractor for PythonExtractor {
    fn language(&self) -> Language {
        Language::Python
    }

    fn file_extensions(&self) -> &[&str] {
        &["py"]
    }

    fn tree_sitter_language(&self) -> tree_sitter::Language {
        tree_sitter_python::LANGUAGE.into()
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
            language: Language::Python,
            parent: None,
        });

        walk_python(source, &tree.root_node(), file_path, file_node_id, None, &mut symbols, &mut raw_edges);
        ExtractResult { symbols, raw_edges }
    }
}

fn walk_python(
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
            "function_definition" => {
                extract_py_function(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
            "class_definition" => {
                extract_py_class(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "import_statement" => {
                extract_py_import(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "import_from_statement" => {
                extract_py_import_from(source, &child, file_path, parent_id, symbols, raw_edges);
            }
            "decorated_definition" => {
                // Recurse into the actual definition inside the decorator
                walk_python(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
            _ => {
                walk_python(source, &child, file_path, parent_id, class_name, symbols, raw_edges);
            }
        }
    }
}

fn extract_py_function(
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

    let return_type = child_by_field(node, "return_type")
        .map(|r| format!(" -> {}", node_text(source, &r)))
        .unwrap_or_default();

    let is_method = class_name.is_some();
    let kind = if is_method && name == "__init__" {
        SymbolKind::Constructor
    } else if is_method {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };

    // Qualify method names with enclosing class for precise resolution
    let qualified_name = match class_name {
        Some(cn) => format!("{cn}.{name}"),
        None => name.to_string(),
    };

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &qualified_name,
        kind,
        node.start_position().row as u32,
    );

    let visibility = if name.starts_with('_') && !name.starts_with("__") {
        Visibility::Private
    } else {
        Visibility::Public
    };

    let body = child_by_field(node, "body");
    let doc_comment = body.as_ref().and_then(|b| extract_python_docstring(source, b));

    symbols.push(SymbolNode {
        id,
        name: qualified_name,
        kind,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("def {name}{params}{return_type}")),
        doc_comment,
        visibility,
        language: Language::Python,
        parent: Some(parent_id),
    });

    // Extract calls within the function body
    if let Some(body) = body {
        extract_py_calls(source, &body, id, class_name, raw_edges);
    }
}

fn extract_py_class(
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
        &file_path.to_string_lossy(),
        name,
        SymbolKind::Class,
        node.start_position().row as u32,
    );

    let body = child_by_field(node, "body");
    let doc_comment = body.as_ref().and_then(|b| extract_python_docstring(source, b));

    symbols.push(SymbolNode {
        id,
        name: name.to_string(),
        kind: SymbolKind::Class,
        file_path: file_path.to_path_buf(),
        line_range: (node.start_position().row as u32, node.end_position().row as u32),
        signature: Some(format!("class {name}")),
        doc_comment,
        visibility: Visibility::Public,
        language: Language::Python,
        parent: Some(parent_id),
    });

    // Extract base classes
    if let Some(args) = child_by_field(node, "superclasses") {
        let mut cursor = args.walk();
        for arg in args.children(&mut cursor) {
            if arg.is_named() {
                let base = node_text(source, &arg);
                raw_edges.push(RawEdge {
                    source: id,
                    kind: EdgeKind::Extends,
                    target_name: base.to_string(),
                    target_module: None,
                    source_line: arg.start_position().row as u32,
                });
            }
        }
    }

    // Recurse into class body for methods — pass class name for qualified method names
    if let Some(body) = body {
        walk_python(source, &body, file_path, id, Some(name), symbols, raw_edges);
    }
}

fn extract_py_import(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let text = node_text(source, node).trim().to_string();

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &text,
        SymbolKind::Import,
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
        language: Language::Python,
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

fn extract_py_import_from(
    source: &[u8],
    node: &tree_sitter::Node,
    file_path: &Path,
    parent_id: NodeId,
    symbols: &mut Vec<SymbolNode>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let text = node_text(source, node).trim().to_string();

    let module_name = child_by_field(node, "module_name")
        .map(|n| node_text(source, &n).to_string());

    let id = NodeId::new(
        &file_path.to_string_lossy(),
        &text,
        SymbolKind::Import,
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
        language: Language::Python,
        parent: Some(parent_id),
    });

    raw_edges.push(RawEdge {
        source: parent_id,
        kind: EdgeKind::Imports,
        target_name: text,
        target_module: module_name,
        source_line: node.start_position().row as u32,
    });
}

fn extract_py_calls(
    source: &[u8],
    node: &tree_sitter::Node,
    parent_id: NodeId,
    class_name: Option<&str>,
    raw_edges: &mut Vec<RawEdge>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "call" {
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
        extract_py_calls(source, &child, parent_id, class_name, raw_edges);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractor::LanguageExtractor;

    fn extract(src: &str) -> (Vec<SymbolNode>, Vec<RawEdge>) {
        let extractor = PythonExtractor;
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&extractor.tree_sitter_language()).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        let r = extractor.extract(src.as_bytes(), &tree, Path::new("test.py"));
        (r.symbols, r.raw_edges)
    }

    fn find<'a>(syms: &'a [SymbolNode], name: &str) -> Option<&'a SymbolNode> {
        syms.iter().find(|s| s.name == name)
    }

    #[test]
    fn extracts_function() {
        let (syms, _) = extract("def add(a, b):\n    return a + b\n");
        let f = find(&syms, "add").expect("add missing");
        assert_eq!(f.kind, SymbolKind::Function);
        assert_eq!(f.visibility, Visibility::Public);
        assert_eq!(f.language, Language::Python);
    }

    #[test]
    fn private_function_starts_with_underscore() {
        let (syms, _) = extract("def _helper():\n    pass\n");
        let f = find(&syms, "_helper").expect("_helper missing");
        assert_eq!(f.visibility, Visibility::Private);
    }

    #[test]
    fn dunder_method_is_public() {
        let src = "class Foo:\n    def __init__(self):\n        pass\n";
        let (syms, _) = extract(src);
        let m = find(&syms, "Foo.__init__").expect("Foo.__init__ missing");
        assert_eq!(m.kind, SymbolKind::Constructor);
        assert_eq!(m.visibility, Visibility::Public);
    }

    #[test]
    fn extracts_class_and_method() {
        let src = "class Foo:\n    def bar(self):\n        return 42\n";
        let (syms, _) = extract(src);
        assert_eq!(find(&syms, "Foo").map(|s| s.kind), Some(SymbolKind::Class));
        let m = find(&syms, "Foo.bar").expect("Foo.bar missing");
        assert_eq!(m.kind, SymbolKind::Method);
    }

    #[test]
    fn extracts_extends_edge() {
        let src = "class Animal:\n    pass\nclass Dog(Animal):\n    pass\n";
        let (_, edges) = extract(src);
        let extends: Vec<_> = edges.iter().filter(|e| e.kind == EdgeKind::Extends).collect();
        assert!(!extends.is_empty(), "expected EXTENDS edge for Dog -> Animal");
    }

    #[test]
    fn emits_calls_edge() {
        let src = "def helper():\n    pass\ndef run():\n    helper()\n";
        let (syms, edges) = extract(src);
        let run = find(&syms, "run").expect("run missing");
        let calls: Vec<&str> = edges.iter()
            .filter(|e| e.source == run.id && e.kind == EdgeKind::Calls)
            .map(|e| e.target_name.as_str())
            .collect();
        assert!(calls.iter().any(|t| *t == "helper"));
    }

    #[test]
    fn qualifies_self_call() {
        let src = "class Foo:\n    def bar(self):\n        self.baz()\n    def baz(self):\n        pass\n";
        let (syms, edges) = extract(src);
        let bar = find(&syms, "Foo.bar").expect("Foo.bar missing");
        let calls: Vec<&str> = edges.iter()
            .filter(|e| e.source == bar.id && e.kind == EdgeKind::Calls)
            .map(|e| e.target_name.as_str())
            .collect();
        assert!(calls.iter().any(|t| *t == "Foo.baz"), "self.baz should resolve to Foo.baz, got: {:?}", calls);
    }

    #[test]
    fn extracts_docstring() {
        let src = "def add(a, b):\n    \"\"\"Add two numbers and return the sum.\"\"\"\n    return a + b\n";
        let (syms, _) = extract(src);
        let f = find(&syms, "add").expect("add missing");
        let doc = f.doc_comment.as_deref().unwrap_or("");
        assert!(doc.contains("Add two numbers"), "doc was: {doc:?}");
    }

    #[test]
    fn extracts_class_docstring() {
        let src = "class Foo:\n    \"\"\"A foo widget.\"\"\"\n    pass\n";
        let (syms, _) = extract(src);
        let c = find(&syms, "Foo").expect("Foo missing");
        assert_eq!(c.doc_comment.as_deref(), Some("A foo widget."));
    }
}
