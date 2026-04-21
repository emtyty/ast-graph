use ast_graph_core::*;
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::info;

/// Resolve raw edges (string-based targets) into concrete edges (NodeId-based).
/// This is the cross-file resolution phase that connects symbols across files.
fn load_path_aliases(root: &std::path::Path) -> Vec<(String, String)> {
    let mut aliases = Vec::new();
    for name in &["tsconfig.json", "jsconfig.json"] {
        let p = root.join(name);
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let bs = '\\';
        let base_url = {
            let key = "\"baseUrl\"";
            if let Some(start) = text.find(key) {
                let after = &text[start + key.len()..];
                if let Some(colon) = after.find(':') {
                    let after = after[colon+1..].trim_start();
                    if after.starts_with('"') {
                        let inner = &after[1..];
                        if let Some(end) = inner.find('"') {
                            root.join(&inner[..end]).to_string_lossy().replace(bs, "/")
                        } else { root.to_string_lossy().replace(bs, "/") }
                    } else { root.to_string_lossy().replace(bs, "/") }
                } else { root.to_string_lossy().replace(bs, "/") }
            } else { root.to_string_lossy().replace(bs, "/") }
        };
        if let Some(paths_start) = text.find("\"paths\"") {
            let after = &text[paths_start..];
            if let Some(brace) = after.find('{') {
                let block = &after[brace..];
                let mut depth = 0i32;
                let depth_end = block.char_indices().find_map(|(i,c)| {
                    match c { '{' => { depth += 1; None } '}' => { depth -= 1; if depth == 0 { Some(i+1) } else { None } } _ => None }
                }).unwrap_or(block.len());
                let block = &block[..depth_end];
                let mut pos = 0usize;
                while pos < block.len() {
                    let Some(rel) = block[pos..].find('"') else { break };
                    let ks = pos + rel + 1;
                    let Some(ke) = block[ks..].find('"') else { break };
                    let key = block[ks..ks+ke].to_string();
                    pos = ks + ke + 1;
                    let Some(ar) = block[pos..].find('[') else { break };
                    let arr = pos + ar + 1;
                    let Some(vq) = block[arr..].find('"') else { break };
                    let vs = arr + vq + 1;
                    let Some(ve) = block[vs..].find('"') else { break };
                    let val = block[vs..vs+ve].to_string();
                    pos = vs + ve + 1;
                    let alias_prefix = key.trim_end_matches("/*").trim_end_matches('*').to_string();
                    let target_suffix = val.trim_end_matches("/*").trim_end_matches('*').to_string();
                    let joined = format!("{}/{}", base_url, target_suffix);
                    let norm = normalize_path_static(&joined);
                    aliases.push((alias_prefix, norm));
                }
            }
        }
        break;
    }
    aliases
}

fn normalize_path_static(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for seg in norm.split('/') {
        match seg { "" | "." => {} ".." => { parts.pop(); } s => parts.push(s), }
    }
    let result = parts.join("/");
    if norm.starts_with('/') { format!("/{result}") } else { result }
}

fn resolve_alias_target(target: &str, aliases: &[(String, String)]) -> Option<String> {
    for (prefix, abs_dir) in aliases {
        if let Some(rest) = target.strip_prefix(prefix.as_str()) {
            let rest = rest.trim_start_matches('/');
            return Some(if rest.is_empty() { abs_dir.clone() } else { format!("{}/{}", abs_dir, rest) });
        }
    }
    None
}

pub fn resolve_edges(graph: &mut CodeGraph, root: &std::path::Path) {
    let path_aliases = load_path_aliases(root);
    if !path_aliases.is_empty() {
        info!("Loaded {} tsconfig path aliases", path_aliases.len());
    }
    let name_index = build_name_index(graph);
    let file_index = build_file_index(graph);

    let raw_edges = std::mem::take(&mut graph.raw_edges);
    let mut resolved = 0;
    let mut unresolved = 0;
    let mut external = 0;
    let mut unresolved_calls: FxHashMap<String, u32> = FxHashMap::default();

    let external_names: &[&str] = &[
        "useCallback", "useState", "useMemo", "useEffect", "useRef", "useContext",
        "useReducer", "useLayoutEffect", "useImperativeHandle", "useDebugValue",
        "setTimeout", "setInterval", "clearTimeout", "clearInterval",
        "requestAnimationFrame", "cancelAnimationFrame",
        "parseInt", "parseFloat", "Number", "Boolean", "String", "isNaN", "isFinite",
        "encodeURIComponent", "decodeURIComponent", "encodeURI", "decodeURI",
        "isEqual", "isNil", "cloneDeep", "debounce", "throttle", "omit", "pick",
        "super", "this",
    ];
    let external_head_prefixes: &[&str] = &[
        "React.", "window.", "document.", "console.", "Math.", "JSON.", "Object.",
        "Array.", "String.", "Number.", "Promise.", "Store.", "_.",
        "new Date",
    ];
    let constant_heads: FxHashSet<String> = graph
        .nodes
        .values()
        .filter(|n| n.kind == SymbolKind::Constant)
        .map(|n| n.name.clone())
        .collect();

    for raw in &raw_edges {
        match raw.kind {
            EdgeKind::Contains => {
                // Contains edges use NodeId encoded as string in target_name
                // These are already resolved during extraction as direct parent-child
                // We handle them by checking if the target NodeId exists
                if let Ok(target_id) = u64::from_str_radix(&raw.target_name, 16) {
                    let target = NodeId(target_id);
                    if graph.nodes.contains_key(&target) {
                        graph.add_edge(Edge {
                            source: raw.source,
                            target,
                            kind: EdgeKind::Contains,
                            source_line: raw.source_line,
                        });
                        resolved += 1;
                        continue;
                    }
                }
                unresolved += 1;
            }
            EdgeKind::Calls => {
                if let Some(targets) = resolve_call_target(&raw.target_name, &name_index) {
                    for target in targets {
                        graph.add_edge(Edge {
                            source: raw.source,
                            target,
                            kind: EdgeKind::Calls,
                            source_line: raw.source_line,
                        });
                        resolved += 1;
                    }
                } else {
                    let t = raw.target_name.as_str();
                    let head = t.split('.').next().unwrap_or(t);
                    let is_ext = external_names.contains(&t)
                        || external_names.contains(&head)
                        || external_head_prefixes.iter().any(|p| t.starts_with(p))
                        || (t.contains('.') && constant_heads.contains(head));
                    if is_ext {
                        external += 1;
                    } else {
                        unresolved += 1;
                        *unresolved_calls.entry(raw.target_name.clone()).or_insert(0) += 1;
                    }
                }
            }
            EdgeKind::Imports => {
                // target_module holds the computed absolute path for relative imports.
                // For aliased imports, resolve via tsconfig paths first.
                let alias_resolved = if raw.target_module.is_none() {
                    resolve_alias_target(&raw.target_name, &path_aliases)
                } else {
                    None
                };
                let targets = raw
                    .target_module
                    .as_deref()
                    .and_then(|abs| resolve_import_by_path(abs, &file_index))
                    .or_else(|| alias_resolved.as_deref().and_then(|abs| resolve_import_by_path(abs, &file_index)))
                    .or_else(|| resolve_import_by_name(&raw.target_name, &name_index));
                if let Some(targets) = targets {
                    for target in targets {
                        graph.add_edge(Edge {
                            source: raw.source,
                            target,
                            kind: EdgeKind::Imports,
                            source_line: raw.source_line,
                        });
                        resolved += 1;
                    }
                } else {
                    unresolved += 1;
                }
            }
            EdgeKind::Extends | EdgeKind::Implements => {
                if let Some(targets) = resolve_type_target(&raw.target_name, &name_index) {
                    for target in targets {
                        graph.add_edge(Edge {
                            source: raw.source,
                            target,
                            kind: raw.kind,
                            source_line: raw.source_line,
                        });
                        resolved += 1;
                    }
                } else {
                    unresolved += 1;
                }
            }
            EdgeKind::References => {
                if let Some(targets) = resolve_type_target(&raw.target_name, &name_index) {
                    for target in targets {
                        graph.add_edge(Edge {
                            source: raw.source,
                            target,
                            kind: EdgeKind::References,
                            source_line: raw.source_line,
                        });
                        resolved += 1;
                    }
                } else {
                    unresolved += 1;
                }
            }
            _ => {
                unresolved += 1;
            }
        }
    }

    graph.refresh_metadata();
    let denom = resolved + unresolved;
    let pct = if denom > 0 { resolved as f64 * 100.0 / denom as f64 } else { 0.0 };
    info!(
        "Resolution complete: {} resolved, {} internal-unresolved, {} external (out of {} raw edges) - internal rate: {:.1}%",
        resolved, unresolved, external, raw_edges.len(), pct,
    );
    if !unresolved_calls.is_empty() {
        let mut top: Vec<(&String, &u32)> = unresolved_calls.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let n = top.len().min(20);
        info!("Top {} unresolved call targets:", n);
        for (name, count) in top.iter().take(n) {
            info!("  {:>6}  {}", count, name);
        }
    }
}

/// Build an index of symbol name -> NodeId for fast lookups.
fn build_name_index(graph: &CodeGraph) -> FxHashMap<String, Vec<NodeId>> {
    let mut index: FxHashMap<String, Vec<NodeId>> = FxHashMap::default();

    for (id, node) in &graph.nodes {
        // Index by full name
        index.entry(node.name.clone()).or_default().push(*id);

        // Also index by the last segment (e.g., "foo::bar::Baz" -> "Baz")
        if let Some(last) = node.name.rsplit("::").next() {
            if last != node.name {
                index.entry(last.to_string()).or_default().push(*id);
            }
        }
        // For dotted names (e.g., "MyClass.method")
        if let Some(last) = node.name.rsplit('.').next() {
            if last != node.name {
                index.entry(last.to_string()).or_default().push(*id);
            }
        }
    }

    index
}

/// Resolve a function call target to NodeIds.
fn resolve_call_target(
    target: &str,
    index: &FxHashMap<String, Vec<NodeId>>,
) -> Option<Vec<NodeId>> {
    // Try exact match first
    if let Some(ids) = index.get(target) {
        return Some(ids.clone());
    }

    // Instance->Class alias: "userService.X" -> "UserService.X"
    if let Some((head, tail)) = target.split_once('.') {
        if let Some(first) = head.chars().next() {
            if first.is_ascii_lowercase() {
                let mut up = String::with_capacity(head.len());
                up.extend(first.to_uppercase());
                up.push_str(&head[first.len_utf8()..]);
                let candidate = format!("{up}.{tail}");
                if let Some(ids) = index.get(&candidate) {
                    return Some(ids.clone());
                }
            }
        }
    }

    // Try the last segment (e.g., "self.process" -> "process")
    let last = target.rsplit('.').next().unwrap_or(target);
    if last != target {
        if let Some(ids) = index.get(last) {
            return Some(ids.clone());
        }
    }

    // Try stripping path prefix (e.g., "crate::utils::parse" -> "parse")
    let last = target.rsplit("::").next().unwrap_or(target);
    if last != target {
        if let Some(ids) = index.get(last) {
            return Some(ids.clone());
        }
    }

    None
}

/// Index File nodes by normalized path (forward slashes, no UNC prefix), with and without extension.
fn build_file_index(graph: &CodeGraph) -> FxHashMap<String, NodeId> {
    let mut index: FxHashMap<String, NodeId> = FxHashMap::default();
    for (id, node) in &graph.nodes {
        if node.kind == SymbolKind::File {
            let raw = node.file_path.to_string_lossy().replace('\\', "/");
            let norm = normalize_path(&raw);
            index.insert(strip_ext(&norm).to_string(), *id);
            index.insert(norm, *id);
        }
    }
    index
}

fn strip_ext(path: &str) -> &str {
    for ext in &[".tsx", ".jsx", ".ts", ".mjs", ".cjs", ".js"] {
        if let Some(s) = path.strip_suffix(ext) { return s; }
    }
    path
}

fn normalize_path(path: &str) -> String {
    let norm = path.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    for seg in norm.split('/') {
        match seg {
            "" | "." => {}
            ".." => { parts.pop(); }
            s => parts.push(s),
        }
    }
    let result = parts.join("/");
    if norm.starts_with('/') { format!("/{result}") } else { result }
}

/// Path-based resolution for relative imports using extension probing.
fn resolve_import_by_path(
    abs_base: &str,
    file_index: &FxHashMap<String, NodeId>,
) -> Option<Vec<NodeId>> {
    let base = normalize_path(abs_base);
    let candidates = [
        base.clone(),
        format!("{base}.js"),  format!("{base}.jsx"),
        format!("{base}.ts"),  format!("{base}.tsx"),
        format!("{base}/index.js"),  format!("{base}/index.jsx"),
        format!("{base}/index.ts"),  format!("{base}/index.tsx"),
    ];
    for c in &candidates {
        if let Some(id) = file_index.get(c.as_str()) { return Some(vec![*id]); }
    }
    None
}

/// Name-based fallback for external packages (lodash, react, etc.).
fn resolve_import_by_name(
    target: &str,
    index: &FxHashMap<String, Vec<NodeId>>,
) -> Option<Vec<NodeId>> {
    if let Some(ids) = index.get(target) { return Some(ids.clone()); }
    let last = target
        .rsplit("::")
        .next()
        .or_else(|| target.rsplit('.').next())
        .or_else(|| target.rsplit('/').next())
        .unwrap_or(target);
    if last != target {
        if let Some(ids) = index.get(last) { return Some(ids.clone()); }
    }
    None
}

/// Resolve a type name to NodeIds (for extends, implements, references).
fn resolve_type_target(
    target: &str,
    index: &FxHashMap<String, Vec<NodeId>>,
) -> Option<Vec<NodeId>> {
    // Strip generic parameters: "Vec<String>" -> "Vec"
    let clean = target.split('<').next().unwrap_or(target).trim();

    if let Some(ids) = index.get(clean) {
        return Some(ids.clone());
    }

    // Try last segment
    let last = clean
        .rsplit("::")
        .next()
        .or_else(|| clean.rsplit('.').next())
        .unwrap_or(clean);

    if last != clean {
        if let Some(ids) = index.get(last) {
            return Some(ids.clone());
        }
    }

    None
}
