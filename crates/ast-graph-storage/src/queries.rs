use anyhow::Result;
use rusqlite::{params, Connection};
use serde_json;

/// Get graph statistics.
pub fn get_stats(conn: &Connection) -> Result<serde_json::Value> {
    let node_count: i64 = conn.query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))?;
    let edge_count: i64 = conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))?;
    let file_count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT file_path) FROM nodes",
        [],
        |r| r.get(0),
    )?;

    // Language breakdown
    let mut stmt = conn.prepare(
        "SELECT language, COUNT(*) as cnt FROM nodes GROUP BY language ORDER BY cnt DESC",
    )?;
    let langs: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "language": row.get::<_, String>(0)?,
                "count": row.get::<_, i64>(1)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Kind breakdown
    let mut stmt = conn.prepare(
        "SELECT kind, COUNT(*) as cnt FROM nodes GROUP BY kind ORDER BY cnt DESC",
    )?;
    let kinds: Vec<serde_json::Value> = stmt
        .query_map([], |row| {
            Ok(serde_json::json!({
                "kind": row.get::<_, String>(0)?,
                "count": row.get::<_, i64>(1)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(serde_json::json!({
        "nodes": node_count,
        "edges": edge_count,
        "files": file_count,
        "languages": langs,
        "kinds": kinds,
    }))
}

/// Find what a function calls (N levels deep) using recursive CTE.
/// Equivalent to: MATCH path = (f)-[:CALLS*1..N]->(target)
pub fn call_chain(conn: &Connection, node_id: &str, max_depth: i32) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "
        WITH RECURSIVE call_tree(id, name, kind, depth, path) AS (
            SELECT n.id, n.name, n.kind, 0, n.name
            FROM nodes n WHERE n.id = ?1

            UNION ALL

            SELECT n2.id, n2.name, n2.kind, ct.depth + 1,
                   ct.path || ' -> ' || n2.name
            FROM call_tree ct
            JOIN edges e ON e.source_id = ct.id AND e.kind = 'CALLS'
            JOIN nodes n2 ON n2.id = e.target_id
            WHERE ct.depth < ?2
        )
        SELECT DISTINCT id, name, kind, depth, path FROM call_tree
        WHERE depth > 0
        ORDER BY depth, name
        ",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![node_id, max_depth], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "depth": row.get::<_, i32>(3)?,
                "path": row.get::<_, String>(4)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Find shortest path between two nodes using recursive CTE with proper index usage.
///
/// The OR pattern `(e.source_id = ps.id OR e.target_id = ps.id)` disables both
/// edge indexes. We split it into two UNION ALL branches so each can use its own
/// index: idx_edges_source for outgoing, idx_edges_target for incoming.
pub fn shortest_path(conn: &Connection, from_id: &str, to_id: &str) -> Result<Vec<serde_json::Value>> {
    let max_depth = 15i32;
    let mut stmt = conn.prepare(
        "
        WITH RECURSIVE path_search(id, name, depth, trail) AS (
            -- Seed: start node
            SELECT n.id, n.name, 0, n.id
            FROM nodes n WHERE n.id = ?1

            UNION ALL

            -- Outgoing edges (uses idx_edges_source)
            SELECT n2.id, n2.name, ps.depth + 1,
                   ps.trail || ',' || n2.id
            FROM path_search ps
            JOIN edges e ON e.source_id = ps.id
            JOIN nodes n2 ON n2.id = e.target_id
            WHERE ps.depth < ?3
              AND ps.trail NOT LIKE '%' || n2.id || '%'

            UNION ALL

            -- Incoming edges (uses idx_edges_target)
            SELECT n2.id, n2.name, ps.depth + 1,
                   ps.trail || ',' || n2.id
            FROM path_search ps
            JOIN edges e ON e.target_id = ps.id
            JOIN nodes n2 ON n2.id = e.source_id
            WHERE ps.depth < ?3
              AND ps.trail NOT LIKE '%' || n2.id || '%'
        )
        SELECT id, name, depth, trail FROM path_search
        WHERE id = ?2
        ORDER BY depth
        LIMIT 1
        ",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![from_id, to_id, max_depth], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "depth": row.get::<_, i32>(2)?,
                "trail": row.get::<_, String>(3)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Find all implementations of a trait/interface.
///
/// Matches exact name OR qualified names (e.g. "Parser" matches "Parser" and "Parser::parse").
/// The original leading-wildcard pattern `'%' || name || '%'` prevented index usage on nodes(name).
/// We now use exact match + trailing wildcard (idx_nodes_name is used for the exact branch).
pub fn find_implementations(conn: &Connection, trait_name: &str) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "
        WITH RECURSIVE impl_tree(impl_id, trait_id, depth) AS (
            -- Direct implementations
            SELECT e.source_id, e.target_id, 1
            FROM edges e
            JOIN nodes t ON t.id = e.target_id
            WHERE e.kind = 'IMPLEMENTS'
              AND (t.name = ?1 OR t.name LIKE ?1 || '::%')

            UNION ALL

            -- Transitive: if A implements B and B implements C, A also implements C
            SELECT e.source_id, it.trait_id, it.depth + 1
            FROM impl_tree it
            JOIN edges e ON e.target_id = it.impl_id AND e.kind = 'IMPLEMENTS'
            WHERE it.depth < 5
        )
        SELECT DISTINCT n.id, n.name, n.kind, n.file_path, n.line_start, it.depth
        FROM impl_tree it
        JOIN nodes n ON n.id = it.impl_id
        ORDER BY it.depth, n.name
        ",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![trait_name], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "file": row.get::<_, String>(3)?,
                "line": row.get::<_, i64>(4)?,
                "depth": row.get::<_, i64>(5)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Find the most connected nodes (architectural hotspots).
pub fn hotspots(conn: &Connection, limit: i32) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "
        SELECT n.id, n.name, n.kind, n.file_path,
               COUNT(DISTINCT e_out.target_id) as outgoing,
               COUNT(DISTINCT e_in.source_id) as incoming,
               COUNT(DISTINCT e_out.target_id) + COUNT(DISTINCT e_in.source_id) as total
        FROM nodes n
        LEFT JOIN edges e_out ON e_out.source_id = n.id
        LEFT JOIN edges e_in ON e_in.target_id = n.id
        WHERE n.kind NOT IN ('File', 'Import')
        GROUP BY n.id
        ORDER BY total DESC
        LIMIT ?1
        ",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![limit], |row| {
            Ok(serde_json::json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "kind": row.get::<_, String>(2)?,
                "file": row.get::<_, String>(3)?,
                "outgoing": row.get::<_, i64>(4)?,
                "incoming": row.get::<_, i64>(5)?,
                "connections": row.get::<_, i64>(6)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Find symbols matching a partial name pattern.
/// Returns nodes ordered by kind priority (Class > Method/Function > others).
pub fn find_symbols(conn: &Connection, pattern: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
    let like_pattern = format!("%{}%", pattern.replace('\'', "''"));
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, file_path, line_start, line_end, signature, parent_id
         FROM nodes
         WHERE name LIKE ?1
           AND kind NOT IN ('File', 'Import')
         ORDER BY
           CASE kind
             WHEN 'Class' THEN 1 WHEN 'Interface' THEN 1 WHEN 'Trait' THEN 1
             WHEN 'Struct' THEN 1 WHEN 'Enum' THEN 1
             WHEN 'Method' THEN 2 WHEN 'Function' THEN 2 WHEN 'Constructor' THEN 2
             ELSE 3
           END,
           length(name)
         LIMIT ?2",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![like_pattern, limit as i64], |row| {
            Ok(serde_json::json!({
                "id":        row.get::<_, String>(0)?,
                "name":      row.get::<_, String>(1)?,
                "kind":      row.get::<_, String>(2)?,
                "file_path": row.get::<_, String>(3)?,
                "line_start":row.get::<_, i64>(4)?,
                "line_end":  row.get::<_, i64>(5)?,
                "signature": row.get::<_, Option<String>>(6)?,
                "parent_id": row.get::<_, Option<String>>(7)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Get direct callers of a node (nodes that have a CALLS edge to this node).
pub fn symbol_callers(conn: &Connection, node_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, n.name, n.kind, n.file_path, n.line_start
         FROM edges e
         JOIN nodes n ON n.id = e.source_id
         WHERE e.target_id = ?1 AND e.kind = 'CALLS'
         ORDER BY n.file_path, n.name",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![node_id], |row| {
            Ok(serde_json::json!({
                "id":        row.get::<_, String>(0)?,
                "name":      row.get::<_, String>(1)?,
                "kind":      row.get::<_, String>(2)?,
                "file_path": row.get::<_, String>(3)?,
                "line":      row.get::<_, i64>(4)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Get direct callees of a node (nodes this node has a CALLS edge to).
pub fn symbol_callees(conn: &Connection, node_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT n.id, n.name, n.kind, n.file_path, n.line_start
         FROM edges e
         JOIN nodes n ON n.id = e.target_id
         WHERE e.source_id = ?1 AND e.kind = 'CALLS'
         ORDER BY n.kind, n.name",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![node_id], |row| {
            Ok(serde_json::json!({
                "id":        row.get::<_, String>(0)?,
                "name":      row.get::<_, String>(1)?,
                "kind":      row.get::<_, String>(2)?,
                "file_path": row.get::<_, String>(3)?,
                "line":      row.get::<_, i64>(4)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Get members of a class/interface/struct (nodes with parent_id = this node).
pub fn symbol_members(conn: &Connection, node_id: &str) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, kind, line_start, line_end, signature, visibility
         FROM nodes
         WHERE parent_id = ?1
           AND kind NOT IN ('Import')
         ORDER BY
           CASE kind
             WHEN 'Constructor' THEN 1
             WHEN 'Method' THEN 2
             WHEN 'Property' THEN 3
             WHEN 'Field' THEN 4
             ELSE 5
           END,
           line_start",
    )?;

    let results: Vec<serde_json::Value> = stmt
        .query_map(params![node_id], |row| {
            Ok(serde_json::json!({
                "id":         row.get::<_, String>(0)?,
                "name":       row.get::<_, String>(1)?,
                "kind":       row.get::<_, String>(2)?,
                "line_start": row.get::<_, i64>(3)?,
                "line_end":   row.get::<_, i64>(4)?,
                "signature":  row.get::<_, Option<String>>(5)?,
                "visibility": row.get::<_, String>(6)?,
            }))
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(results)
}

/// Run an arbitrary SQL query and return results as JSON.
pub fn run_sql(conn: &Connection, sql: &str) -> Result<Vec<serde_json::Value>> {
    let mut stmt = conn.prepare(sql)?;
    let column_count = stmt.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
        .collect();

    let mut rows = Vec::new();
    let mut result_rows = stmt.query([])?;

    while let Some(row) = result_rows.next()? {
        let mut obj = serde_json::Map::new();
        for (i, col_name) in column_names.iter().enumerate() {
            let val = match row.get_ref(i) {
                Ok(rusqlite::types::ValueRef::Null) => serde_json::Value::Null,
                Ok(rusqlite::types::ValueRef::Integer(n)) => serde_json::json!(n),
                Ok(rusqlite::types::ValueRef::Real(f)) => serde_json::json!(f),
                Ok(rusqlite::types::ValueRef::Text(s)) => {
                    serde_json::Value::String(String::from_utf8_lossy(s).to_string())
                }
                Ok(rusqlite::types::ValueRef::Blob(b)) => {
                    serde_json::Value::String(format!("<blob {} bytes>", b.len()))
                }
                Err(_) => serde_json::Value::Null,
            };
            obj.insert(col_name.clone(), val);
        }
        rows.push(serde_json::Value::Object(obj));
    }

    Ok(rows)
}
