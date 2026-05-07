use anyhow::Result;
use ast_graph_storage::GraphStorage;

pub fn run(limit: usize, storage: &dyn GraphStorage) -> Result<()> {
    let sql = format!(
        "SELECT p.id, p.name, p.file_path, p.line_start,
                (SELECT COUNT(*) FROM edges e
                 WHERE e.target_id = p.id AND e.kind = 'STEP_IN_PROCESS') AS steps
         FROM nodes p
         WHERE p.kind = 'Process'
         ORDER BY steps DESC
         LIMIT {limit}"
    );
    let cypher = format!(
        "MATCH (p:Symbol {{kind:'Process'}}) \
         OPTIONAL MATCH (s)-[:STEP_IN_PROCESS]->(p) \
         WITH p, count(s) AS steps \
         RETURN p.id, p.name, p.file_path, p.line_start, steps \
         ORDER BY steps DESC \
         LIMIT {limit}"
    );

    let query = if storage.backend_name() == "sqlite" {
        sql
    } else {
        cypher
    };
    let rows = storage.run_raw_query(&query)?;
    if rows.is_empty() {
        println!("No processes found. Try `ast-graph scan` first — processes are traced from entry points (main, route handlers, test fns) and require resolved CALLS edges.");
        return Ok(());
    }
    println!("Processes ({}):\n", rows.len());
    for r in rows {
        let name = field_str(&r, "name", "p.name");
        let file = field_str(&r, "file_path", "p.file_path");
        let line = field_i64(&r, "line_start", "p.line_start");
        let steps = field_i64(&r, "steps", "steps");
        println!("  {:<48}  {} step(s)  {}:{}", name, steps, file, line);
    }
    Ok(())
}

fn field_str(v: &serde_json::Value, a: &str, b: &str) -> String {
    v.get(a)
        .or_else(|| v.get(b))
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string()
}

fn field_i64(v: &serde_json::Value, a: &str, b: &str) -> i64 {
    v.get(a)
        .or_else(|| v.get(b))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}
