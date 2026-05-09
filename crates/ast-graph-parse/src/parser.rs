use std::path::Path;

use anyhow::Result;
use ast_graph_core::*;
use rayon::prelude::*;
use tracing::info;

use crate::extractor::ExtractResult;
use crate::incremental::{build_hash_map, discover_files};
use crate::lang::get_extractor;

/// Options controlling parse behavior.
#[derive(Debug, Clone)]
pub struct ParseOptions {
    /// When true, populate `SymbolNode.doc_comment` from preceding comments
    /// (or Python docstrings) during extraction.  Default: true.
    pub extract_doc_comments: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self { extract_doc_comments: true }
    }
}

/// Parse an entire project directory and build a CodeGraph (default options).
pub fn parse_project(root: &Path) -> Result<CodeGraph> {
    parse_project_with_options(root, &ParseOptions::default())
}

/// Parse an entire project directory using the given options.
pub fn parse_project_with_options(root: &Path, options: &ParseOptions) -> Result<CodeGraph> {
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
                Ok(mut extract) => {
                    apply_options(&mut extract, options);
                    Some((file.path.clone(), extract))
                }
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

    // File-based routing pre-pass — emits Route nodes for Next.js App/Pages
    // Router, Remix, Nuxt, SvelteKit conventions before resolution runs.
    crate::routes_filebased::detect_filebased_routes(&mut graph);

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

    let mut result = extractor.extract(&source, &tree, path);

    // Post-pass: route extraction. Adds Route nodes + HandlesRoute raw edges
    // for any HTTP route declarations in this file.
    let mut extra_symbols: Vec<SymbolNode> = Vec::new();
    let mut extra_edges: Vec<RawEdge> = Vec::new();
    crate::routes::extract_routes(
        &source,
        path,
        language,
        &result.symbols,
        &mut extra_symbols,
        &mut extra_edges,
    );
    result.symbols.extend(extra_symbols);
    result.raw_edges.extend(extra_edges);

    Ok(result)
}

/// Apply post-extraction options to an ExtractResult.  Currently strips
/// doc_comment from every symbol when `extract_doc_comments` is disabled.
fn apply_options(extract: &mut ExtractResult, options: &ParseOptions) {
    if !options.extract_doc_comments {
        for symbol in extract.symbols.iter_mut() {
            symbol.doc_comment = None;
        }
    }
}

/// Incrementally update a graph: only re-parse changed files (default options).
pub fn incremental_update(graph: &mut CodeGraph, root: &Path) -> Result<IncrementalStats> {
    incremental_update_with_options(graph, root, &ParseOptions::default())
}

/// Incrementally update a graph using the given options.
pub fn incremental_update_with_options(
    graph: &mut CodeGraph,
    root: &Path,
    options: &ParseOptions,
) -> Result<IncrementalStats> {
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
            parse_single_file(path, *lang).ok().map(|mut r| {
                apply_options(&mut r, options);
                (path.clone(), r)
            })
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
