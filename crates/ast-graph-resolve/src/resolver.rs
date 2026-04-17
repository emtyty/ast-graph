use ast_graph_core::*;
use rustc_hash::FxHashMap;
use tracing::info;

/// Resolve raw edges (string-based targets) into concrete edges (NodeId-based).
/// This is the cross-file resolution phase that connects symbols across files.
pub fn resolve_edges(graph: &mut CodeGraph) {
    // Build name index: name -> Vec<NodeId>
    let name_index = build_name_index(graph);

    let raw_edges = std::mem::take(&mut graph.raw_edges);
    let mut resolved = 0;
    let mut unresolved = 0;

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
                    unresolved += 1;
                }
            }
            EdgeKind::Imports => {
                if let Some(targets) = resolve_import_target(&raw.target_name, &name_index) {
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
    info!(
        "Resolution complete: {} resolved, {} unresolved out of {} raw edges",
        resolved,
        unresolved,
        raw_edges.len()
    );
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

/// Resolve an import target to NodeIds.
fn resolve_import_target(
    target: &str,
    index: &FxHashMap<String, Vec<NodeId>>,
) -> Option<Vec<NodeId>> {
    // Imports often match File or Module nodes
    if let Some(ids) = index.get(target) {
        return Some(ids.clone());
    }

    // Try just the last component
    let last = target
        .rsplit("::")
        .next()
        .or_else(|| target.rsplit('.').next())
        .or_else(|| target.rsplit('/').next())
        .unwrap_or(target);

    if last != target {
        if let Some(ids) = index.get(last) {
            return Some(ids.clone());
        }
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
