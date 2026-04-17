use anyhow::Result;
use ast_graph_storage::GraphStorage;

pub fn run(name: &str, depth: i32, storage: &dyn GraphStorage) -> Result<()> {
    // Disambiguate by name via the backend's built-in symbol search.
    let matches = storage.find_symbols(name, 5)?;
    let funcs: Vec<&serde_json::Value> = matches
        .iter()
        .filter(|m| {
            let k = m["kind"].as_str().unwrap_or("");
            k == "Function" || k == "Method" || k == "Constructor"
        })
        .collect();

    if funcs.is_empty() {
        println!("No function matching '{}' found. Run 'ast-graph scan .' first.", name);
        return Ok(());
    }

    if funcs.len() > 1 {
        println!("Multiple matches found:");
        for r in &funcs {
            println!(
                "  {} ({}) - {}",
                r["name"].as_str().unwrap_or("?"),
                r["kind"].as_str().unwrap_or("?"),
                r["id"].as_str().unwrap_or("?"),
            );
        }
        println!();
    }

    let node_id = funcs[0]["id"].as_str().unwrap_or("");
    let node_name = funcs[0]["name"].as_str().unwrap_or(name);

    println!("Call chain from '{}' (depth {}):\n", node_name, depth);

    let chain = storage.call_chain(node_id, depth)?;

    if chain.is_empty() {
        println!("  (no outgoing calls found)");
    } else {
        for entry in &chain {
            let d = entry["depth"].as_i64().unwrap_or(0);
            let indent = "  ".repeat(d as usize);
            let line_hint = entry["call_line"]
                .as_i64()
                .filter(|l| *l > 0)
                .map(|l| format!(" @L{}", l))
                .unwrap_or_default();
            println!(
                "{}{} ({}){}",
                indent,
                entry["name"].as_str().unwrap_or("?"),
                entry["kind"].as_str().unwrap_or("?"),
                line_hint,
            );
        }
    }

    Ok(())
}
