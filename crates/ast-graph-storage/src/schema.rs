use anyhow::Result;
use rusqlite::Connection;
use tracing::info;

/// Create all tables and indexes for the code graph.
/// Foreign-key constraints enforce referential integrity:
///   - edges.source_id / target_id CASCADE DELETE when the referenced node is removed
///   - nodes.parent_id SET NULL when the parent node is removed
pub fn create_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        );

        CREATE TABLE IF NOT EXISTS nodes (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            kind        TEXT NOT NULL,
            file_path   TEXT NOT NULL,
            line_start  INTEGER NOT NULL,
            line_end    INTEGER NOT NULL,
            signature   TEXT,
            doc_comment TEXT,
            visibility  TEXT NOT NULL,
            language    TEXT NOT NULL,
            parent_id   TEXT REFERENCES nodes(id) ON DELETE SET NULL
        );

        CREATE TABLE IF NOT EXISTS edges (
            source_id   TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            target_id   TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            kind        TEXT NOT NULL,
            PRIMARY KEY (source_id, target_id, kind)
        );

        CREATE TABLE IF NOT EXISTS file_hashes (
            file_path   TEXT PRIMARY KEY,
            hash        BLOB NOT NULL
        );

        -- Single-column lookups
        CREATE INDEX IF NOT EXISTS idx_nodes_name     ON nodes(name);
        CREATE INDEX IF NOT EXISTS idx_nodes_kind     ON nodes(kind);
        CREATE INDEX IF NOT EXISTS idx_nodes_file     ON nodes(file_path);
        CREATE INDEX IF NOT EXISTS idx_nodes_language ON nodes(language);
        CREATE INDEX IF NOT EXISTS idx_nodes_parent   ON nodes(parent_id);

        -- Compound index: file + kind (used by file-subgraph and filter queries)
        CREATE INDEX IF NOT EXISTS idx_nodes_file_kind ON nodes(file_path, kind);

        -- Edge traversal indexes (both directions)
        CREATE INDEX IF NOT EXISTS idx_edges_source        ON edges(source_id);
        CREATE INDEX IF NOT EXISTS idx_edges_target        ON edges(target_id);
        CREATE INDEX IF NOT EXISTS idx_edges_kind          ON edges(kind);

        -- Compound: existence check and kind-filtered traversal
        CREATE INDEX IF NOT EXISTS idx_edges_source_target ON edges(source_id, target_id);
        CREATE INDEX IF NOT EXISTS idx_edges_source_kind   ON edges(source_id, kind);
        CREATE INDEX IF NOT EXISTS idx_edges_target_kind   ON edges(target_id, kind);
        ",
    )?;

    info!("SQLite schema ready");
    Ok(())
}

/// Migrate an existing database to the current schema version.
///
/// SQLite does not support ALTER TABLE ADD CONSTRAINT, so we recreate
/// the tables using the rename-recreate-copy pattern.
/// Safe to call on a fresh DB (no-op if already at version 2).
pub fn migrate_schema(conn: &Connection) -> Result<()> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if version >= 2 {
        return Ok(());
    }

    info!("Migrating database schema to version 2 (adding FK constraints)...");

    conn.execute_batch("
        PRAGMA foreign_keys = OFF;
        BEGIN;

        -- Recreate nodes with self-referencing FK on parent_id
        CREATE TABLE IF NOT EXISTS nodes_v2 (
            id          TEXT PRIMARY KEY,
            name        TEXT NOT NULL,
            kind        TEXT NOT NULL,
            file_path   TEXT NOT NULL,
            line_start  INTEGER NOT NULL,
            line_end    INTEGER NOT NULL,
            signature   TEXT,
            doc_comment TEXT,
            visibility  TEXT NOT NULL,
            language    TEXT NOT NULL,
            parent_id   TEXT REFERENCES nodes_v2(id) ON DELETE SET NULL
        );
        INSERT OR IGNORE INTO nodes_v2 SELECT * FROM nodes;
        DROP TABLE nodes;
        ALTER TABLE nodes_v2 RENAME TO nodes;

        -- Recreate edges with CASCADE FK constraints.
        -- Filter out dangling edges (referencing deleted nodes) before inserting.
        CREATE TABLE IF NOT EXISTS edges_v2 (
            source_id   TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            target_id   TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            kind        TEXT NOT NULL,
            PRIMARY KEY (source_id, target_id, kind)
        );
        INSERT OR IGNORE INTO edges_v2
            SELECT * FROM edges
            WHERE source_id IN (SELECT id FROM nodes)
              AND target_id IN (SELECT id FROM nodes);
        DROP TABLE edges;
        ALTER TABLE edges_v2 RENAME TO edges;

        -- Record the new version
        INSERT OR REPLACE INTO schema_version VALUES (2);

        COMMIT;
        PRAGMA foreign_keys = ON;
    ")?;

    // Recreate indexes (they were dropped when tables were dropped)
    create_schema(conn)?;

    info!("Schema migration to version 2 complete");
    Ok(())
}

/// Drop all data (truncate tables).
pub fn clear_database(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        DELETE FROM edges;
        DELETE FROM nodes;
        DELETE FROM file_hashes;
        ",
    )?;
    info!("Database cleared");
    Ok(())
}
