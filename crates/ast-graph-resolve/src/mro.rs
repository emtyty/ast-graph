//! Method Resolution Order (MRO) — C3 linearization for Python.
//!
//! Resolves `super().method()` calls in Python by walking the class's
//! C3-linearized ancestor list and picking the first definition. Used as a
//! post-resolution pass so it benefits from already-built EXTENDS edges.

use ast_graph_core::*;
use rustc_hash::FxHashMap;

/// Compute the C3 linearization of a class given a precomputed parents map.
/// Returns the list of class NodeIds in MRO order, starting with `class_id`
/// itself. Returns `None` if linearization is impossible (inconsistent
/// hierarchy — same diamond conflict that real Python rejects).
pub fn c3_linearize(
    class_id: NodeId,
    parents: &FxHashMap<NodeId, Vec<NodeId>>,
) -> Option<Vec<NodeId>> {
    fn linearize(
        class: NodeId,
        parents: &FxHashMap<NodeId, Vec<NodeId>>,
        memo: &mut FxHashMap<NodeId, Option<Vec<NodeId>>>,
    ) -> Option<Vec<NodeId>> {
        if let Some(cached) = memo.get(&class) {
            return cached.clone();
        }
        // Mark in-progress with `None` to detect cycles.
        memo.insert(class, None);
        let direct_parents = parents.get(&class).cloned().unwrap_or_default();
        // Start with linearizations of every direct parent + the parent list
        // itself. Then merge per the C3 rule: take the head of the first list
        // that doesn't appear in the tail of any other list.
        let mut to_merge: Vec<Vec<NodeId>> = Vec::new();
        for p in &direct_parents {
            match linearize(*p, parents, memo) {
                Some(l) => to_merge.push(l),
                None => {
                    memo.insert(class, None);
                    return None;
                }
            }
        }
        if !direct_parents.is_empty() {
            to_merge.push(direct_parents.clone());
        }
        let mut result = vec![class];
        loop {
            // Drop empty lists.
            to_merge.retain(|l| !l.is_empty());
            if to_merge.is_empty() {
                break;
            }
            // Find the first head that isn't in any tail.
            let head = to_merge.iter().find_map(|list| {
                let candidate = list[0];
                let in_any_tail = to_merge
                    .iter()
                    .any(|other| other.iter().skip(1).any(|x| *x == candidate));
                if in_any_tail {
                    None
                } else {
                    Some(candidate)
                }
            });
            let Some(picked) = head else {
                // Inconsistent hierarchy — fail.
                memo.insert(class, None);
                return None;
            };
            result.push(picked);
            for list in to_merge.iter_mut() {
                if list.first() == Some(&picked) {
                    list.remove(0);
                }
            }
        }
        memo.insert(class, Some(result.clone()));
        Some(result)
    }

    let mut memo = FxHashMap::default();
    linearize(class_id, parents, &mut memo)
}

/// Build a `class_id -> [direct_parent_ids]` map from EXTENDS edges, for
/// Python-language classes only (other languages don't need C3 — single
/// inheritance fits the existing first-wins resolution).
pub fn python_parents_map(graph: &CodeGraph) -> FxHashMap<NodeId, Vec<NodeId>> {
    let mut parents: FxHashMap<NodeId, Vec<NodeId>> = FxHashMap::default();
    for (src, edges) in &graph.adjacency {
        let src_node = match graph.nodes.get(src) {
            Some(n) => n,
            None => continue,
        };
        if src_node.language != Language::Python || src_node.kind != SymbolKind::Class {
            continue;
        }
        for e in edges {
            if e.kind == EdgeKind::Extends {
                if let Some(tgt) = graph.nodes.get(&e.target) {
                    if tgt.kind == SymbolKind::Class
                        && tgt.language == Language::Python
                    {
                        parents.entry(*src).or_default().push(e.target);
                    }
                }
            }
        }
    }
    parents
}

/// Returns the enclosing class of a method by walking up the parent chain
/// once. Returns `None` if `node_id` is not a method or its parent isn't a class.
pub fn enclosing_class(graph: &CodeGraph, node_id: NodeId) -> Option<NodeId> {
    let node = graph.nodes.get(&node_id)?;
    let parent_id = node.parent?;
    let parent = graph.nodes.get(&parent_id)?;
    if matches!(
        parent.kind,
        SymbolKind::Class | SymbolKind::Struct | SymbolKind::Trait | SymbolKind::Interface
    ) {
        Some(parent_id)
    } else {
        None
    }
}

/// Find a method named `method_name` in the MRO of `class_id`, skipping the
/// class itself (the calling class). Returns the first matching NodeId found
/// in C3 order.
pub fn lookup_super_method(
    graph: &CodeGraph,
    class_id: NodeId,
    method_name: &str,
    mro_cache: &mut FxHashMap<NodeId, Option<Vec<NodeId>>>,
    parents: &FxHashMap<NodeId, Vec<NodeId>>,
) -> Option<NodeId> {
    let mro = match mro_cache.get(&class_id) {
        Some(cached) => cached.clone(),
        None => {
            let computed = c3_linearize(class_id, parents);
            mro_cache.insert(class_id, computed.clone());
            computed
        }
    };
    let mro = mro?;
    // Skip the calling class itself (first entry). For each ancestor class,
    // scan the graph for a method whose `parent` is that class and whose
    // simple-name matches. We use the parent field rather than CONTAINS edges
    // because CONTAINS-from-parent is synthesized only at save time.
    for cls_id in mro.iter().skip(1) {
        for (id, node) in &graph.nodes {
            if node.parent != Some(*cls_id) {
                continue;
            }
            if !matches!(node.kind, SymbolKind::Method | SymbolKind::Function) {
                continue;
            }
            if member_simple_name(&node.name) == method_name {
                return Some(*id);
            }
        }
    }
    None
}

fn member_simple_name(qualified: &str) -> &str {
    qualified.rsplit('.').next().unwrap_or(qualified)
}

/// Strip `super().method` / `super(Class, self).method` down to just `method`.
/// Returns `None` if the target isn't a `super()` call.
pub fn extract_super_method(target: &str) -> Option<&str> {
    // Must start with `super(` and contain `).` followed by the method name.
    if !target.starts_with("super(") {
        return None;
    }
    let close = target.find(").")?;
    let after = &target[close + 2..];
    // Reject chained accesses (`super().a.b`) — too ambiguous to resolve here.
    if after.contains('.') || after.contains('(') {
        return None;
    }
    if after.is_empty() {
        return None;
    }
    Some(after)
}

