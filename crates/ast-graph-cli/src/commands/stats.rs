use anyhow::Result;
use std::path::Path;

pub fn run(db_path: Option<&Path>) -> Result<()> {
    let canon = Path::new(".").canonicalize()?;
    let db_file = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| ast_graph_storage::default_db_path(&canon));
    let conn = ast_graph_storage::open_db(&db_file)?;

    let stats = ast_graph_storage::get_stats(&conn)?;

    println!("Graph Statistics:");
    println!("  Nodes: {}", stats["nodes"]);
    println!("  Edges: {}", stats["edges"]);
    println!("  Files: {}", stats["files"]);

    if let Some(langs) = stats["languages"].as_array() {
        println!("\n  By Language:");
        for lang in langs {
            println!(
                "    {:<15} {}",
                lang["language"].as_str().unwrap_or("?"),
                lang["count"]
            );
        }
    }

    if let Some(kinds) = stats["kinds"].as_array() {
        println!("\n  By Kind:");
        for kind in kinds {
            println!(
                "    {:<15} {}",
                kind["kind"].as_str().unwrap_or("?"),
                kind["count"]
            );
        }
    }

    Ok(())
}
