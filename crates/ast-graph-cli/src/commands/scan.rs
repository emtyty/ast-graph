use anyhow::Result;
use ast_graph_storage::GraphStorage;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use tracing::info;

pub fn run(path: &str, storage: &dyn GraphStorage, clean: bool) -> Result<()> {
    let path = Path::new(path);

    if clean {
        storage.clear()?;
        info!("Cleared existing graph data");
    }

    let pb = ProgressBar::new_spinner();
    pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}")?);
    pb.set_message("Scanning source files...");

    let mut graph = ast_graph_parse::parse_project(path)?;
    pb.set_message(format!(
        "Parsed {} files, {} symbols",
        graph.metadata.total_files, graph.metadata.total_nodes
    ));

    pb.set_message("Resolving cross-file references...");
    ast_graph_resolve::resolve_edges(&mut graph);
    pb.set_message(format!(
        "Resolved: {} nodes, {} edges",
        graph.metadata.total_nodes, graph.metadata.total_edges
    ));

    pb.set_message(format!("Saving to {}...", storage.backend_name()));
    let (node_count, edge_count) = storage.save_graph(&graph)?;

    pb.finish_with_message(format!(
        "Done! {} nodes, {} edges saved to {}",
        node_count,
        edge_count,
        storage.backend_name()
    ));

    println!("\nGraph Summary:");
    println!("  Backend:   {}", storage.backend_name());
    println!("  Files:     {}", graph.metadata.total_files);
    println!("  Nodes:     {}", node_count);
    println!("  Edges:     {}", edge_count);
    println!("  Languages: {:?}", graph.metadata.languages);

    Ok(())
}
