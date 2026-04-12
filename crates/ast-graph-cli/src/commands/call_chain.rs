use anyhow::Result;
use std::path::Path;

pub fn run(name: &str, depth: i32, db_path: Option<&Path>) -> Result<()> {
    let canon = Path::new(".").canonicalize()?;
    let db_file = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| ast_graph_storage::default_db_path(&canon));
    let conn = ast_graph_storage::open_db(&db_file)?;

    // Find the node ID by name
    let results = ast_graph_storage::run_sql(
        &conn,
        &format!(
            "SELECT id, name, kind FROM nodes WHERE name LIKE '%{}%' AND kind IN ('Function', 'Method') LIMIT 5",
            name.replace('\'', "''")
        ),
    )?;

    if results.is_empty() {
        println!("No function matching '{}' found. Run 'ast-graph scan .' first.", name);
        return Ok(());
    }

    if results.len() > 1 {
        println!("Multiple matches found:");
        for r in &results {
            println!(
                "  {} ({}) - {}",
                r["name"].as_str().unwrap_or("?"),
                r["kind"].as_str().unwrap_or("?"),
                r["id"].as_str().unwrap_or("?"),
            );
        }
        println!();
    }

    let node_id = results[0]["id"].as_str().unwrap_or("");
    let node_name = results[0]["name"].as_str().unwrap_or(name);

    println!("Call chain from '{}' (depth {}):\n", node_name, depth);

    let chain = ast_graph_storage::call_chain(&conn, node_id, depth)?;

    if chain.is_empty() {
        println!("  (no outgoing calls found)");
    } else {
        for entry in &chain {
            let d = entry["depth"].as_i64().unwrap_or(0);
            let indent = "  ".repeat(d as usize);
            println!(
                "{}{} ({})",
                indent,
                entry["name"].as_str().unwrap_or("?"),
                entry["kind"].as_str().unwrap_or("?"),
            );
        }
    }

    Ok(())
}
