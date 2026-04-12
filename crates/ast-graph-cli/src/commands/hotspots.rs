use anyhow::Result;
use std::path::Path;

pub fn run(limit: i32, db_path: Option<&Path>) -> Result<()> {
    let canon = Path::new(".").canonicalize()?;
    let db_file = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| ast_graph_storage::default_db_path(&canon));
    let conn = ast_graph_storage::open_db(&db_file)?;

    let results = ast_graph_storage::hotspots(&conn, limit)?;

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
