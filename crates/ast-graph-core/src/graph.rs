use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::relation::{Edge, RawEdge};
use crate::symbol::{Language, NodeId, SymbolNode};

/// The central graph data structure holding all compressed AST information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGraph {
    pub project_root: PathBuf,
    pub nodes: FxHashMap<NodeId, SymbolNode>,
    pub adjacency: FxHashMap<NodeId, Vec<Edge>>,
    pub reverse_adj: FxHashMap<NodeId, Vec<Edge>>,
    pub file_index: FxHashMap<PathBuf, Vec<NodeId>>,
    pub file_hashes: FxHashMap<PathBuf, [u8; 32]>,
    pub raw_edges: Vec<RawEdge>,
    pub metadata: GraphMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphMetadata {
    pub total_files: usize,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub languages: Vec<Language>,
}

impl CodeGraph {
    pub fn new(project_root: PathBuf) -> Self {
        Self {
            project_root,
            nodes: FxHashMap::default(),
            adjacency: FxHashMap::default(),
            reverse_adj: FxHashMap::default(),
            file_index: FxHashMap::default(),
            file_hashes: FxHashMap::default(),
            raw_edges: Vec::new(),
            metadata: GraphMetadata {
                total_files: 0,
                total_nodes: 0,
                total_edges: 0,
                languages: Vec::new(),
            },
        }
    }

    pub fn add_node(&mut self, node: SymbolNode) {
        let id = node.id;
        let file_path = node.file_path.clone();
        self.nodes.insert(id, node);
        self.file_index.entry(file_path).or_default().push(id);
    }

    pub fn add_edge(&mut self, edge: Edge) {
        let fwd = edge.clone();
        let rev = Edge {
            source: edge.target,
            target: edge.source,
            kind: edge.kind,
            source_line: edge.source_line,
        };
        self.adjacency.entry(edge.source).or_default().push(fwd);
        self.reverse_adj.entry(edge.target).or_default().push(rev);
    }

    pub fn add_raw_edge(&mut self, raw: RawEdge) {
        self.raw_edges.push(raw);
    }

    /// Remove all nodes and edges belonging to a specific file.
    pub fn remove_file(&mut self, file_path: &Path) {
        if let Some(node_ids) = self.file_index.remove(file_path) {
            for id in &node_ids {
                self.nodes.remove(id);
                self.adjacency.remove(id);
                self.reverse_adj.remove(id);
            }
            // Remove edges pointing to removed nodes
            for edges in self.adjacency.values_mut() {
                edges.retain(|e| !node_ids.contains(&e.target));
            }
            for edges in self.reverse_adj.values_mut() {
                edges.retain(|e| !node_ids.contains(&e.target));
            }
        }
        self.file_hashes.remove(file_path);
    }

    /// Get outgoing edges from a node.
    pub fn outgoing(&self, id: &NodeId) -> &[Edge] {
        self.adjacency.get(id).map_or(&[], |v| v.as_slice())
    }

    /// Get incoming edges to a node.
    pub fn incoming(&self, id: &NodeId) -> &[Edge] {
        self.reverse_adj.get(id).map_or(&[], |v| v.as_slice())
    }

    /// Find nodes by name (substring match).
    pub fn find_by_name(&self, query: &str) -> Vec<&SymbolNode> {
        self.nodes
            .values()
            .filter(|n| n.name.contains(query))
            .collect()
    }

    /// Update metadata counts.
    pub fn refresh_metadata(&mut self) {
        self.metadata.total_files = self.file_index.len();
        self.metadata.total_nodes = self.nodes.len();
        self.metadata.total_edges = self.adjacency.values().map(|v| v.len()).sum();

        let mut langs: Vec<Language> = self
            .nodes
            .values()
            .map(|n| n.language)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        langs.sort_by_key(|l| l.as_str());
        self.metadata.languages = langs;
    }
}
