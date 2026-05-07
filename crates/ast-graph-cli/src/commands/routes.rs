use anyhow::Result;
use ast_graph_storage::GraphStorage;

pub fn run(limit: usize, storage: &dyn GraphStorage) -> Result<()> {
    // Backend-native query. SQLite gets a SQL query; FalkorDB gets a Cypher one.
    let sql = format!(
        "SELECT n.id, n.name, n.file_path, n.line_start,
                COUNT(DISTINCT e.source_id) AS handlers
         FROM nodes n
         LEFT JOIN edges e ON e.target_id = n.id AND e.kind = 'HANDLES_ROUTE'
         WHERE n.kind = 'Route'
         GROUP BY n.id
         ORDER BY n.name
         LIMIT {limit}"
    );
    let cypher = format!(
        "MATCH (r:Symbol {{kind:'Route'}}) \
         OPTIONAL MATCH (h:Symbol)-[:HANDLES_ROUTE]->(r) \
         WITH r, count(DISTINCT h) AS handlers \
         RETURN r.id, r.name, r.file_path, r.line_start, handlers \
         ORDER BY r.name \
         LIMIT {limit}"
    );

    let query = if storage.backend_name() == "sqlite" {
        sql
    } else {
        cypher
    };
    let rows = storage.run_raw_query(&query)?;
    if rows.is_empty() {
        println!("No routes found in the graph.");
        return Ok(());
    }
    println!("Routes ({}):\n", rows.len());
    for r in rows {
        let name = field_str(&r, "name", "r.name");
        let file = field_str(&r, "file_path", "r.file_path");
        let line = field_i64(&r, "line_start", "r.line_start");
        let handlers = field_i64(&r, "handlers", "handlers");
        println!("  {:<32}  {} handler(s)  {}:{}", name, handlers, file, line);
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
