use anyhow::Result;
use ast_graph_storage::GraphStorage;

pub fn run(query: &str, limit: usize, storage: &dyn GraphStorage) -> Result<()> {
    let results = storage.search_symbols(query, limit)?;
    if results.is_empty() {
        println!("No matches for: {}", query);
        return Ok(());
    }
    println!("Top {} matches for: {}\n", results.len(), query);
    for (i, r) in results.iter().enumerate() {
        let name = r.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = r.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
        let file = r.get("file_path").and_then(|v| v.as_str()).unwrap_or("?");
        let line = r.get("line_start").and_then(|v| v.as_i64()).unwrap_or(0);
        let score = r.get("score").and_then(|v| v.as_f64()).unwrap_or(0.0);
        println!(
            "{:>3}. [{:>11}] {} ({}:{}) — score {:.3}",
            i + 1,
            kind,
            name,
            file,
            line,
            score
        );
        if let Some(doc) = r.get("doc_comment").and_then(|v| v.as_str()) {
            if !doc.is_empty() {
                let preview = doc.lines().next().unwrap_or("").trim();
                if !preview.is_empty() {
                    println!("     {}", preview);
                }
            }
        }
    }
    Ok(())
}
