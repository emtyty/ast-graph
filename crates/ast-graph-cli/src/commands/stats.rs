use anyhow::Result;
use ast_graph_storage::GraphStorage;

pub fn run(storage: &dyn GraphStorage) -> Result<()> {
    let stats = storage.get_stats()?;

    println!("Graph Statistics ({}):", storage.backend_name());
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
