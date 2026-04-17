use anyhow::Result;
use ast_graph_storage::GraphStorage;

pub fn run(query: &str, storage: &dyn GraphStorage) -> Result<()> {
    let results = storage.run_raw_query(query)?;

    if results.is_empty() {
        println!("(no results)");
    } else {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    Ok(())
}
