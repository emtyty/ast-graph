//! Process tracing — identifies entry points and emits Process nodes plus
//! STEP_IN_PROCESS edges along CALLS chains rooted at each entry.
//!
//! Heuristic-based by design. Entry-point detection covers:
//!   - `main`, `Main`, `Program.Main`, `__main__` shims
//!   - Symbols with attached `Route` nodes (HTTP handlers)
//!   - Functions named `test_*` / annotated as tests
//!
//! The walk is depth-bounded (default 6 hops) and step-bounded (default 50
//! symbols) to keep processes from ballooning on large codebases.

use ast_graph_core::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

/// Tunables for process tracing — kept conservative to avoid noise on large
/// codebases. Override via `trace_processes_with_limits` if needed.
pub const DEFAULT_MAX_DEPTH: u32 = 6;
pub const DEFAULT_MAX_STEPS: usize = 50;

/// Build Process nodes + edges and add them to the graph in place.
///
/// Returns (process_count, step_edge_count).
pub fn trace_processes(graph: &mut CodeGraph) -> (usize, usize) {
    trace_processes_with_limits(graph, DEFAULT_MAX_DEPTH, DEFAULT_MAX_STEPS)
}

pub fn trace_processes_with_limits(
    graph: &mut CodeGraph,
    max_depth: u32,
    max_steps: usize,
) -> (usize, usize) {
    let entry_points = detect_entry_points(graph);
    if entry_points.is_empty() {
        return (0, 0);
    }

    let mut process_count = 0;
    let mut step_count = 0;

    // Pre-build: file path lookup for each NodeId so processes inherit a path.
    let id_to_node: FxHashMap<NodeId, (String, u32, Language)> = graph
        .nodes
        .iter()
        .map(|(id, n)| {
            (
                *id,
                (
                    n.file_path.to_string_lossy().to_string(),
                    n.line_range.0,
                    n.language,
                ),
            )
        })
        .collect();

    let mut new_nodes: Vec<SymbolNode> = Vec::new();
    let mut new_edges: Vec<Edge> = Vec::new();

    for (entry_id, entry_label) in entry_points {
        let entry_meta = match id_to_node.get(&entry_id) {
            Some(m) => m.clone(),
            None => continue,
        };

        // Process node — name is "<entry symbol name>" or "Process: <label>"
        // for synthetic labels like route handlers.
        let entry_name = graph
            .nodes
            .get(&entry_id)
            .map(|n| n.name.clone())
            .unwrap_or_else(|| "<unknown>".to_string());
        let proc_name = format!("Process: {}", entry_label.unwrap_or(entry_name.clone()));
        let proc_id = NodeId::new(
            &entry_meta.0,
            &proc_name,
            SymbolKind::Process,
            entry_meta.1,
        );
        // Skip if a process for this entry was already emitted (idempotent re-runs).
        if graph.nodes.contains_key(&proc_id) {
            continue;
        }

        new_nodes.push(SymbolNode {
            id: proc_id,
            name: proc_name.clone(),
            kind: SymbolKind::Process,
            file_path: entry_meta.0.clone().into(),
            line_range: (entry_meta.1, entry_meta.1),
            signature: Some(format!("process rooted at {}", entry_name)),
            doc_comment: None,
            visibility: Visibility::Public,
            language: entry_meta.2,
            parent: None,
        });
        process_count += 1;

        // EntryPointOf edge: entry --> process
        new_edges.push(Edge {
            source: entry_id,
            target: proc_id,
            kind: EdgeKind::EntryPointOf,
            source_line: entry_meta.1,
            confidence: CONFIDENCE_EXACT,
        });

        // BFS along CALLS edges. Each visited node becomes a STEP_IN_PROCESS
        // edge from the symbol -> process. step_index encoded in source_line.
        let mut queue: VecDeque<(NodeId, u32)> = VecDeque::new();
        queue.push_back((entry_id, 0));
        let mut seen: FxHashSet<NodeId> = FxHashSet::default();
        seen.insert(entry_id);
        let mut step_index: u32 = 1;

        // Entry itself is step 1.
        new_edges.push(Edge {
            source: entry_id,
            target: proc_id,
            kind: EdgeKind::StepInProcess,
            source_line: step_index,
            confidence: CONFIDENCE_EXACT,
        });
        step_count += 1;
        step_index += 1;

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_depth || step_count >= max_steps * (process_count) {
                continue;
            }
            let outs = graph.outgoing(&current).to_vec();
            for e in outs {
                if e.kind != EdgeKind::Calls {
                    continue;
                }
                if !seen.insert(e.target) {
                    continue;
                }
                if step_index as usize > max_steps {
                    break;
                }
                new_edges.push(Edge {
                    source: e.target,
                    target: proc_id,
                    kind: EdgeKind::StepInProcess,
                    source_line: step_index,
                    confidence: e.confidence,
                });
                step_count += 1;
                step_index += 1;
                queue.push_back((e.target, depth + 1));
            }
        }
    }

    for n in new_nodes {
        graph.add_node(n);
    }
    for e in new_edges {
        graph.add_edge(e);
    }
    graph.refresh_metadata();
    (process_count, step_count)
}

/// Detect entry points from the graph. Returns `(node_id, optional label)`
/// pairs — the label is used for the resulting Process node's name when the
/// entry symbol itself is anonymous (e.g. a route handler that's a closure).
pub fn detect_entry_points(graph: &CodeGraph) -> Vec<(NodeId, Option<String>)> {
    let mut entries: Vec<(NodeId, Option<String>)> = Vec::new();
    let mut seen: FxHashSet<NodeId> = FxHashSet::default();

    // 1) Symbols that handle a route — every route is an entry point.
    for (src, edges) in &graph.adjacency {
        for e in edges {
            if e.kind == EdgeKind::HandlesRoute {
                if seen.insert(*src) {
                    let label = graph.nodes.get(&e.target).map(|n| n.name.clone());
                    entries.push((*src, label));
                }
            }
        }
    }

    // 2) `main` / `Main` / `Program.Main` / `__main__`-style entry points.
    for (id, n) in &graph.nodes {
        if !matches!(
            n.kind,
            SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
        ) {
            continue;
        }
        let name = n.name.as_str();
        let last = name.rsplit('.').next().unwrap_or(name);
        let last = last.rsplit("::").next().unwrap_or(last);
        let is_main = matches!(last, "main" | "Main") || name == "Program.Main";
        let is_test = last.starts_with("test_") || last.starts_with("Test")
            || last.ends_with("Test") || last.ends_with("_test");
        if (is_main || is_test) && seen.insert(*id) {
            entries.push((*id, None));
        }
    }

    entries
}
