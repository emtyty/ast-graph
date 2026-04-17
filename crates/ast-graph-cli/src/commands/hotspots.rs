use anyhow::Result;
use ast_graph_storage::GraphStorage;

pub fn run(limit: i32, storage: &dyn GraphStorage) -> Result<()> {
    let results = storage.hotspots(limit)?;

    if results.is_empty() {
        println!("No hotspots found. Run 'ast-graph scan .' first.");
        return Ok(());
    }

    println!(
        "{:<30} {:<15} {:>8} {:>8} {:>8}",
        "Name", "Kind", "Out", "In", "Total"
    );
    println!("{}", "-".repeat(75));

    for r in &results {
        println!(
            "{:<30} {:<15} {:>8} {:>8} {:>8}",
            truncate(r["name"].as_str().unwrap_or("?"), 30),
            r["kind"].as_str().unwrap_or("?"),
            r["outgoing"],
            r["incoming"],
            r["connections"],
        );
    }

    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max - 3])
    }
}
