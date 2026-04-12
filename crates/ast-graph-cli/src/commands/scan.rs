use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use tracing::info;

pub fn run(path: &str, db_path: Option<&Path>, clean: bool) -> Result<()> {
    let path = Path::new(path);
    let canon = path.canonicalize()?;

    // Open database
    let db_file = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| ast_graph_storage::default_db_path(&canon));
    let conn = ast_graph_storage::open_db(&db_file)?;

    if clean {
        ast_graph_storage::clear_database(&conn)?;
        info!("Cleared existing graph data");
    }

    // Parse the project
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")?,
    );
    pb.set_message("Scanning source files...");

    let mut graph = ast_graph_parse::parse_project(path)?;
    pb.set_message(format!(
        "Parsed {} files, {} symbols",
        graph.metadata.total_files, graph.metadata.total_nodes
    ));

    // Resolve cross-file edges
    pb.set_message("Resolving cross-file references...");
    ast_graph_resolve::resolve_edges(&mut graph);
    pb.set_message(format!(
        "Resolved: {} nodes, {} edges",
        graph.metadata.total_nodes, graph.metadata.total_edges
    ));

    // Save to SQLite
    pb.set_message("Saving to database...");
    let (node_count, edge_count) = ast_graph_storage::save_graph(&conn, &graph)?;

    pb.finish_with_message(format!(
        "Done! {} nodes, {} edges saved to {}",
        node_count,
        edge_count,
        db_file.display()
    ));

    println!("\nGraph Summary:");
    println!("  Files:     {}", graph.metadata.total_files);
    println!("  Nodes:     {}", graph.metadata.total_nodes);
    println!("  Edges:     {}", graph.metadata.total_edges);
    println!("  Languages: {:?}", graph.metadata.languages);
    println!("  Database:  {}", db_file.display());

    Ok(())
}
