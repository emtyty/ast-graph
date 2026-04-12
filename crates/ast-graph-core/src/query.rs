use crate::symbol::{NodeId, SymbolKind, SymbolNode, Language};
use crate::graph::CodeGraph;

/// Builder for filtering nodes in the graph.
pub struct GraphQuery<'a> {
    graph: &'a CodeGraph,
    kind_filter: Option<SymbolKind>,
    language_filter: Option<Language>,
    file_filter: Option<String>,
    name_filter: Option<String>,
}

impl<'a> GraphQuery<'a> {
    pub fn new(graph: &'a CodeGraph) -> Self {
        Self {
            graph,
            kind_filter: None,
            language_filter: None,
            file_filter: None,
            name_filter: None,
        }
    }

    pub fn kind(mut self, kind: SymbolKind) -> Self {
        self.kind_filter = Some(kind);
        self
    }

    pub fn language(mut self, lang: Language) -> Self {
        self.language_filter = Some(lang);
        self
    }

    pub fn file(mut self, path: impl Into<String>) -> Self {
        self.file_filter = Some(path.into());
        self
    }

    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name_filter = Some(name.into());
        self
    }

    pub fn execute(&self) -> Vec<&SymbolNode> {
        self.graph
            .nodes
            .values()
            .filter(|node| {
                if let Some(kind) = &self.kind_filter {
                    if node.kind != *kind {
                        return false;
                    }
                }
                if let Some(lang) = &self.language_filter {
                    if node.language != *lang {
                        return false;
                    }
                }
                if let Some(file) = &self.file_filter {
                    if !node.file_path.to_string_lossy().contains(file.as_str()) {
                        return false;
                    }
                }
                if let Some(name) = &self.name_filter {
                    if !node.name.contains(name.as_str()) {
                        return false;
                    }
                }
                true
            })
            .collect()
    }
}

/// Get all nodes within N hops of a starting node.
pub fn neighbors(graph: &CodeGraph, start: &NodeId, depth: usize) -> Vec<NodeId> {
    use std::collections::{HashSet, VecDeque};

    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();
    visited.insert(*start);
    queue.push_back((*start, 0));

    while let Some((current, d)) = queue.pop_front() {
        if d >= depth {
            continue;
        }
        for edge in graph.outgoing(&current) {
            if visited.insert(edge.target) {
                queue.push_back((edge.target, d + 1));
            }
        }
        for edge in graph.incoming(&current) {
            if visited.insert(edge.target) {
                queue.push_back((edge.target, d + 1));
            }
        }
    }

    visited.into_iter().collect()
}
