use anyhow::Result;
use rusqlite::Connection;
use tracing::info;

/// Current schema version. Bumped when the table layout changes.
/// v1: initial
/// v2: FK constraints (CASCADE on edges, SET NULL on nodes.parent_id)
/// v3: edges.source_line column + expanded PK (source_id, target_id, kind, source_line)
pub const SCHEMA_VERSION: i64 = 3;

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
            source_id    TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            target_id    TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
            kind         TEXT NOT NULL,
            source_line  INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (source_id, target_id, kind, source_line)
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

        -- Line-level edge index: \"all call sites from this source ordered by line\"
        CREATE INDEX IF NOT EXISTS idx_edges_source_line   ON edges(source_id, source_line);
        ",
    )?;

    info!("SQLite schema ready");
    Ok(())
}

/// Migrate an existing database to the current schema version.
///
/// SQLite does not support ALTER TABLE ADD CONSTRAINT, so we recreate
/// tables using the rename-recreate-copy pattern.
pub fn migrate_schema(conn: &Connection) -> Result<()> {
    let version: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);

    if version >= SCHEMA_VERSION {
        return Ok(());
    }

    // Fresh database: `create_schema` already built the latest layout,
    // so there's nothing to migrate. Just stamp the current version.
    let node_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
        .unwrap_or(0);
    let edge_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))
        .unwrap_or(0);
    if version == 0 && node_count == 0 && edge_count == 0 {
        conn.execute(
            "INSERT OR REPLACE INTO schema_version VALUES (?1)",
            [SCHEMA_VERSION],
        )?;
        return Ok(());
    }

    // v1 -> v2: add FK constraints.
    if version < 2 {
        info!("Migrating database schema to version 2 (adding FK constraints)...");
        conn.execute_batch("
            PRAGMA foreign_keys = OFF;
            BEGIN;

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

            INSERT OR REPLACE INTO schema_version VALUES (2);

            COMMIT;
            PRAGMA foreign_keys = ON;
        ")?;
        info!("Schema migration to v2 complete");
    }

    // v2 -> v3: add source_line column + expand edges PK.
    if version < 3 {
        info!("Migrating database schema to version 3 (adding edges.source_line)...");
        conn.execute_batch("
            PRAGMA foreign_keys = OFF;
            BEGIN;

            CREATE TABLE IF NOT EXISTS edges_v3 (
                source_id    TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
                target_id    TEXT NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
                kind         TEXT NOT NULL,
                source_line  INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (source_id, target_id, kind, source_line)
            );
            -- Copy old rows; source_line defaults to 0 since the information
            -- was never captured pre-v3. Re-scan to populate accurate lines.
            INSERT OR IGNORE INTO edges_v3 (source_id, target_id, kind, source_line)
                SELECT source_id, target_id, kind, 0 FROM edges;
            DROP TABLE edges;
            ALTER TABLE edges_v3 RENAME TO edges;

            INSERT OR REPLACE INTO schema_version VALUES (3);

            COMMIT;
            PRAGMA foreign_keys = ON;
        ")?;
        info!("Schema migration to v3 complete (re-scan to populate source_line)");
    }

    // Recreate indexes (dropped with tables)
    create_schema(conn)?;
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
