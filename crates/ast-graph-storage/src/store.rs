use anyhow::Result;
use ast_graph_core::*;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use tracing::info;

/// Open (or create) the SQLite database at the given path.
/// Runs schema creation and any pending migrations automatically.
pub fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA foreign_keys = ON;
        PRAGMA cache_size = -64000;
        ",
    )?;
    crate::schema::create_schema(&conn)?;
    crate::schema::migrate_schema(&conn)?;
    info!("Opened database at {}", path.display());
    Ok(conn)
}

/// Open an in-memory database (for testing or ephemeral use).
pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    crate::schema::create_schema(&conn)?;
    Ok(conn)
}

/// Default database path: `.ast-graph/graph.db` in the project root.
pub fn default_db_path(project_root: &Path) -> PathBuf {
    let dir = project_root.join(".ast-graph");
    std::fs::create_dir_all(&dir).ok();
    dir.join("graph.db")
}

/// Persist an entire CodeGraph into SQLite.
pub fn save_graph(conn: &Connection, graph: &CodeGraph) -> Result<(usize, usize)> {
    // Defer FK checks so node insertion order doesn't matter for parent_id refs
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
            // Only reference parent if it actually exists in the graph
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

    // Upsert edges — skip edges referencing nodes not in the graph
    let mut edge_count = 0;
    let mut skipped = 0;
    {
        let mut stmt = tx.prepare_cached(
            "INSERT OR IGNORE INTO edges (source_id, target_id, kind)
             VALUES (?1, ?2, ?3)",
        )?;
        for edges in graph.adjacency.values() {
            for edge in edges {
                if !graph.nodes.contains_key(&edge.source)
                    || !graph.nodes.contains_key(&edge.target)
                {
                    skipped += 1;
                    continue;
                }
                stmt.execute(params![
                    edge.source.to_string(),
                    edge.target.to_string(),
                    edge.kind.as_neo4j_type(),
                ])?;
                edge_count += 1;
            }
        }
    }
    if skipped > 0 {
        info!("Skipped {} edges with dangling node references", skipped);
    }

    // Save file hashes
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

/// Remove all nodes and edges belonging to a specific file.
/// With FK CASCADE constraints the edge deletions are automatic, but we keep
/// the explicit DELETE as a safety net for any pre-migration databases.
pub fn remove_file_nodes(conn: &Connection, file_path: &str) -> Result<()> {
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

/// Load a CodeGraph from SQLite (reconstruct in-memory graph from stored data).
/// Used by the server to avoid re-parsing on startup when a DB already exists.
pub fn load_graph(conn: &Connection, project_root: PathBuf) -> Result<CodeGraph> {
    let mut graph = CodeGraph::new(project_root);

    // Load nodes
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

    // Load edges
    let mut stmt = conn.prepare("SELECT source_id, target_id, kind FROM edges")?;
    let edges: Vec<Edge> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .filter_map(|(src, tgt, kind_str)| {
            let source = NodeId::from_hex(&src)?;
            let target = NodeId::from_hex(&tgt)?;
            let kind = EdgeKind::from_neo4j_type(&kind_str)?;
            Some(Edge { source, target, kind })
        })
        .collect();

    for edge in edges {
        graph.add_edge(edge);
    }

    // Load file hashes
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

/// Load file hashes from the database (used for incremental scan).
pub fn load_file_hashes(conn: &Connection) -> Result<rustc_hash::FxHashMap<PathBuf, [u8; 32]>> {
    let mut map = rustc_hash::FxHashMap::default();
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
