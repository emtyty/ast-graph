use anyhow::Result;
use std::path::Path;

pub fn run(sql: &str, db_path: Option<&Path>) -> Result<()> {
    let canon = Path::new(".").canonicalize()?;
    let db_file = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| ast_graph_storage::default_db_path(&canon));
    let conn = ast_graph_storage::open_db(&db_file)?;

    let results = ast_graph_storage::run_sql(&conn, sql)?;

    if results.is_empty() {
        println!("(no results)");
    } else {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    Ok(())
}
