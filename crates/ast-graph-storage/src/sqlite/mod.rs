mod schema;

use anyhow::Result;
use ast_graph_core::*;
use rusqlite::{params, Connection};
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::info;

use crate::GraphStorage;

pub use schema::clear_database;

/// SQLite-backed storage. `rusqlite::Connection` is not `Sync`, so we wrap
/// it in a `Mutex` to satisfy the `GraphStorage: Send + Sync` bound.
pub struct SqliteStorage {
    conn: Mutex<Connection>,
}

impl SqliteStorage {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            PRAGMA foreign_keys = ON;
            PRAGMA cache_size = -64000;
            ",
        )?;
        schema::create_schema(&conn)?;
        schema::migrate_schema(&conn)?;
        info!("Opened SQLite database at {}", path.display());
        Ok(Self { conn: Mutex::new(conn) })
    }

    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::create_schema(&conn)?;
        Ok(Self { conn: Mutex::new(conn) })
    }
}

impl GraphStorage for SqliteStorage {
    fn backend_name(&self) -> &'static str {
        "sqlite"
    }

    fn save_graph(&self, graph: &CodeGraph) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("PRAGMA defer_foreign_keys = ON")?;
        let tx = conn.unchecked_transaction()?;

        // Upsert nodes
        let mut node_count = 0;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO nodes (id, name, kind, file_path, line_start, line_end, signature, doc_comment, visibility, language, parent_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for node in graph.nodes.values() {
                let parent_id = node
                    .parent
                    .filter(|p| graph.nodes.contains_key(p))
                    .map(|p| p.to_string());
                stmt.execute(params![
                    node.id.to_string(),
                    node.name,
                    node.kind.as_neo4j_label(),
                    node.file_path.to_string_lossy().to_string(),
                    node.line_range.0 as i64,
                    node.line_range.1 as i64,
                    node.signature,
                    node.doc_comment,
                    format!("{:?}", node.visibility),
                    node.language.as_str(),
                    parent_id,
                ])?;
                node_count += 1;
            }
        }

        // Upsert edges — skip edges referencing nodes not in the graph.
        // Count only rows actually inserted (INSERT OR IGNORE returns 0 when
        // a duplicate by the composite PK is skipped), so the reported count
        // matches `SELECT COUNT(*) FROM edges`.
        let mut edge_count = 0;
        let mut skipped = 0;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO edges (source_id, target_id, kind, source_line)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for edges in graph.adjacency.values() {
                for edge in edges {
                    if !graph.nodes.contains_key(&edge.source)
                        || !graph.nodes.contains_key(&edge.target)
                    {
                        skipped += 1;
                        continue;
                    }
                    let affected = stmt.execute(params![
                        edge.source.to_string(),
                        edge.target.to_string(),
                        edge.kind.as_neo4j_type(),
                        edge.source_line as i64,
                    ])?;
                    edge_count += affected;
                }
            }
        }

        // Synthesize CONTAINS edges for parent-child containment that the
        // parser didn't emit directly (e.g. class → method). Parser-emitted
        // CONTAINS (file → top-level items) is deduped by the composite PK.
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO edges (source_id, target_id, kind, source_line)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for node in graph.nodes.values() {
                let parent = match node.parent {
                    Some(p) if graph.nodes.contains_key(&p) => p,
                    _ => continue,
                };
                let affected = stmt.execute(params![
                    parent.to_string(),
                    node.id.to_string(),
                    "CONTAINS",
                    node.line_range.0 as i64,
                ])?;
                edge_count += affected;
            }
        }
        if skipped > 0 {
            info!("Skipped {} edges with dangling node references", skipped);
        }

        // File hashes
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO file_hashes (file_path, hash) VALUES (?1, ?2)",
            )?;
            for (path, hash) in &graph.file_hashes {
                stmt.execute(params![
                    path.to_string_lossy().to_string(),
                    hash.as_slice(),
                ])?;
            }
        }

        tx.commit()?;
        info!("Saved {} nodes, {} edges to SQLite", node_count, edge_count);
        Ok((node_count, edge_count))
    }

    fn remove_file_nodes(&self, file_path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM edges WHERE source_id IN (SELECT id FROM nodes WHERE file_path = ?1)
             OR target_id IN (SELECT id FROM nodes WHERE file_path = ?1)",
            params![file_path],
        )?;
        tx.execute("DELETE FROM nodes WHERE file_path = ?1", params![file_path])?;
        tx.execute("DELETE FROM file_hashes WHERE file_path = ?1", params![file_path])?;
        tx.commit()?;
        Ok(())
    }

    fn load_graph(&self, project_root: PathBuf) -> Result<CodeGraph> {
        let conn = self.conn.lock().unwrap();
        let mut graph = CodeGraph::new(project_root);

        // Nodes
        let mut stmt = conn.prepare(
            "SELECT id, name, kind, file_path, line_start, line_end, signature, doc_comment, visibility, language, parent_id
             FROM nodes",
        )?;
        let nodes: Vec<SymbolNode> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(id_str, name, kind_str, file_path, line_start, line_end,
                          signature, doc_comment, vis_str, lang_str, parent_str)| {
                let id = NodeId::from_hex(&id_str)?;
                let kind = SymbolKind::from_label(&kind_str)?;
                let language = Language::from_str(&lang_str)?;
                let visibility = Visibility::from_debug_str(&vis_str);
                let parent = parent_str.as_deref().and_then(NodeId::from_hex);
                Some(SymbolNode {
                    id,
                    name,
                    kind,
                    file_path: PathBuf::from(file_path),
                    line_range: (line_start as u32, line_end as u32),
                    signature,
                    doc_comment,
                    visibility,
                    language,
                    parent,
                })
            })
            .collect();
        for node in nodes {
            graph.add_node(node);
        }

        // Edges
        let mut stmt = conn.prepare("SELECT source_id, target_id, kind, source_line FROM edges")?;
        let edges: Vec<Edge> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(src, tgt, kind_str, line)| {
                let source = NodeId::from_hex(&src)?;
                let target = NodeId::from_hex(&tgt)?;
                let kind = EdgeKind::from_neo4j_type(&kind_str)?;
                Some(Edge { source, target, kind, source_line: line as u32 })
            })
            .collect();
        for edge in edges {
            graph.add_edge(edge);
        }

        // File hashes
        let mut stmt = conn.prepare("SELECT file_path, hash FROM file_hashes")?;
        let hash_rows: Vec<(String, Vec<u8>)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        for (path, hash_bytes) in hash_rows {
            if hash_bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&hash_bytes);
                graph.file_hashes.insert(PathBuf::from(path), arr);
            }
        }

        graph.refresh_metadata();
        info!(
            "Loaded graph from DB: {} nodes, {} edges, {} files",
            graph.metadata.total_nodes, graph.metadata.total_edges, graph.metadata.total_files
        );
        Ok(graph)
    }

    fn load_file_hashes(&self) -> Result<FxHashMap<PathBuf, [u8; 32]>> {
        let conn = self.conn.lock().unwrap();
        let mut map = FxHashMap::default();
        let mut stmt = conn.prepare("SELECT file_path, hash FROM file_hashes")?;
        let rows: Vec<(String, Vec<u8>)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?
            .filter_map(|r| r.ok())
            .collect();
        for (path, hash_bytes) in rows {
            if hash_bytes.len() == 32 {
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&hash_bytes);
                map.insert(PathBuf::from(path), arr);
            }
        }
        Ok(map)
    }

    fn clear(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        schema::clear_database(&conn)
    }

    fn get_stats(&self) -> Result<serde_json::Value> {
        let conn = self.conn.lock().unwrap();
        let node_count: i64 = conn.query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))?;
        let edge_count: i64 = conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))?;
        let file_count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT file_path) FROM nodes",
            [],
            |r| r.get(0),
        )?;

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
            "backend": "sqlite",
            "nodes": node_count,
            "edges": edge_count,
            "files": file_count,
            "languages": langs,
            "kinds": kinds,
        }))
    }

    fn call_chain(&self, node_id: &str, max_depth: i32) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "
            WITH RECURSIVE call_tree(id, name, kind, depth, path, call_line) AS (
                SELECT n.id, n.name, n.kind, 0, n.name, 0
                FROM nodes n WHERE n.id = ?1

                UNION ALL

                SELECT n2.id, n2.name, n2.kind, ct.depth + 1,
                       ct.path || ' -> ' || n2.name,
                       e.source_line
                FROM call_tree ct
                JOIN edges e ON e.source_id = ct.id AND e.kind = 'CALLS'
                JOIN nodes n2 ON n2.id = e.target_id
                WHERE ct.depth < ?2
            )
            SELECT DISTINCT id, name, kind, depth, path, call_line FROM call_tree
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
                    "call_line": row.get::<_, i64>(5)?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    fn shortest_path(&self, from_id: &str, to_id: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let max_depth = 15i32;
        let mut stmt = conn.prepare(
            "
            WITH RECURSIVE path_search(id, name, depth, trail) AS (
                SELECT n.id, n.name, 0, n.id
                FROM nodes n WHERE n.id = ?1

                UNION ALL

                SELECT n2.id, n2.name, ps.depth + 1,
                       ps.trail || ',' || n2.id
                FROM path_search ps
                JOIN edges e ON e.source_id = ps.id
                JOIN nodes n2 ON n2.id = e.target_id
                WHERE ps.depth < ?3
                  AND ps.trail NOT LIKE '%' || n2.id || '%'

                UNION ALL

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

    fn find_implementations(&self, trait_name: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "
            WITH RECURSIVE impl_tree(impl_id, trait_id, depth) AS (
                SELECT e.source_id, e.target_id, 1
                FROM edges e
                JOIN nodes t ON t.id = e.target_id
                WHERE e.kind = 'IMPLEMENTS'
                  AND (t.name = ?1 OR t.name LIKE ?1 || '::%')

                UNION ALL

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

    fn hotspots(&self, limit: i32) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
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

    fn find_symbols(&self, pattern: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
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

    fn symbol_callers(&self, node_id: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.name, n.kind, n.file_path, n.line_start, e.source_line
             FROM edges e
             JOIN nodes n ON n.id = e.source_id
             WHERE e.target_id = ?1 AND e.kind = 'CALLS'
             ORDER BY n.file_path, e.source_line",
        )?;

        let results: Vec<serde_json::Value> = stmt
            .query_map(params![node_id], |row| {
                Ok(serde_json::json!({
                    "id":             row.get::<_, String>(0)?,
                    "name":           row.get::<_, String>(1)?,
                    "kind":           row.get::<_, String>(2)?,
                    "file_path":      row.get::<_, String>(3)?,
                    "line":           row.get::<_, i64>(4)?,
                    "call_site_line": row.get::<_, i64>(5)?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    fn symbol_callees(&self, node_id: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT n.id, n.name, n.kind, n.file_path, n.line_start, e.source_line
             FROM edges e
             JOIN nodes n ON n.id = e.target_id
             WHERE e.source_id = ?1 AND e.kind = 'CALLS'
             ORDER BY e.source_line, n.name",
        )?;

        let results: Vec<serde_json::Value> = stmt
            .query_map(params![node_id], |row| {
                Ok(serde_json::json!({
                    "id":             row.get::<_, String>(0)?,
                    "name":           row.get::<_, String>(1)?,
                    "kind":           row.get::<_, String>(2)?,
                    "file_path":      row.get::<_, String>(3)?,
                    "line":           row.get::<_, i64>(4)?,
                    "call_site_line": row.get::<_, i64>(5)?,
                }))
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(results)
    }

    fn symbol_members(&self, node_id: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
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

    fn run_raw_query(&self, query: &str) -> Result<Vec<serde_json::Value>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(query)?;
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
}
