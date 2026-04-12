use std::path::Path;

use anyhow::Result;
use ast_graph_core::*;
use rayon::prelude::*;
use tracing::info;

use crate::extractor::ExtractResult;
use crate::incremental::{build_hash_map, discover_files};
use crate::lang::get_extractor;

/// Parse an entire project directory and build a CodeGraph.
pub fn parse_project(root: &Path) -> Result<CodeGraph> {
    let root = root.canonicalize()?;
    let mut graph = CodeGraph::new(root.clone());

    info!("Discovering source files in {}", root.display());
    let files = discover_files(&root);
    info!("Found {} source files", files.len());

    graph.file_hashes = build_hash_map(&files);

    let results: Vec<(std::path::PathBuf, ExtractResult)> = files
        .par_iter()
        .filter_map(|file| {
            let result = parse_single_file(&file.path, file.language);
            match result {
                Ok(extract) => Some((file.path.clone(), extract)),
                Err(e) => {
                    tracing::warn!("Failed to parse {}: {}", file.path.display(), e);
                    None
                }
            }
        })
        .collect();

    for (_, result) in results {
        for symbol in result.symbols {
            graph.add_node(symbol);
        }
        for raw_edge in result.raw_edges {
            graph.add_raw_edge(raw_edge);
        }
    }

    graph.refresh_metadata();
    info!(
        "Graph built: {} nodes, {} raw edges, {} files, languages: {:?}",
        graph.metadata.total_nodes,
        graph.raw_edges.len(),
        graph.metadata.total_files,
        graph.metadata.languages,
    );

    Ok(graph)
}

/// Parse a single file and extract symbols.
pub fn parse_single_file(path: &Path, language: Language) -> Result<ExtractResult> {
    let source = std::fs::read(path)?;
    let extractor = get_extractor(language);

    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&extractor.tree_sitter_language())?;

    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse {}", path.display()))?;

    Ok(extractor.extract(&source, &tree, path))
}

/// Incrementally update a graph: only re-parse changed files.
pub fn incremental_update(graph: &mut CodeGraph, root: &Path) -> Result<IncrementalStats> {
    let root = root.canonicalize()?;
    let files = discover_files(&root);
    let current_hashes = build_hash_map(&files);

    let prev_state = IncrementalState::from_hashes(graph.file_hashes.clone());
    let changes = prev_state.detect_changes(&current_hashes);

    let stats = IncrementalStats {
        added: changes.added.len(),
        modified: changes.modified.len(),
        removed: changes.removed.len(),
        unchanged: files.len() - changes.added.len() - changes.modified.len(),
    };

    for path in &changes.removed {
        graph.remove_file(path);
    }
    for path in &changes.modified {
        graph.remove_file(path);
    }

    let to_parse: Vec<_> = changes
        .added
        .iter()
        .chain(changes.modified.iter())
        .filter_map(|path| {
            let ext = path.extension()?.to_str()?;
            let lang = Language::from_extension(ext)?;
            Some((path.clone(), lang))
        })
        .collect();

    let results: Vec<(std::path::PathBuf, ExtractResult)> = to_parse
        .par_iter()
        .filter_map(|(path, lang)| {
            parse_single_file(path, *lang)
                .ok()
                .map(|r| (path.clone(), r))
        })
        .collect();

    for (_, result) in results {
        for symbol in result.symbols {
            graph.add_node(symbol);
        }
        for raw_edge in result.raw_edges {
            graph.add_raw_edge(raw_edge);
        }
    }

    graph.file_hashes = current_hashes;
    graph.refresh_metadata();

    Ok(stats)
}

#[derive(Debug)]
pub struct IncrementalStats {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub unchanged: usize,
}
