use anyhow::Result;
use ast_graph_core::CodeGraph;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

pub mod sqlite;

#[cfg(feature = "falkor")]
pub mod falkor;

pub use sqlite::SqliteStorage;

#[cfg(feature = "falkor")]
pub use falkor::{FalkorConfig, FalkorStorage};

/// Backend-agnostic interface for the code graph store.
///
/// Both SQLite and FalkorDB (Cypher) backends implement this trait so the CLI
/// and query layers are independent of the physical storage engine.
pub trait GraphStorage: Send + Sync {
    /// Persist the whole graph. Returns (node_count, edge_count) inserted.
    fn save_graph(&self, graph: &CodeGraph) -> Result<(usize, usize)>;

    /// Reconstruct a `CodeGraph` from storage.
    fn load_graph(&self, project_root: PathBuf) -> Result<CodeGraph>;

    /// Load per-file hashes (for incremental scanning).
    fn load_file_hashes(&self) -> Result<FxHashMap<PathBuf, [u8; 32]>>;

    /// Remove every node, edge, and file-hash belonging to a single file.
    fn remove_file_nodes(&self, file_path: &str) -> Result<()>;

    /// Drop all data from the store.
    fn clear(&self) -> Result<()>;

    fn get_stats(&self) -> Result<serde_json::Value>;
    fn call_chain(&self, node_id: &str, max_depth: i32) -> Result<Vec<serde_json::Value>>;
    fn shortest_path(&self, from_id: &str, to_id: &str) -> Result<Vec<serde_json::Value>>;
    fn find_implementations(&self, trait_name: &str) -> Result<Vec<serde_json::Value>>;
    fn hotspots(&self, limit: i32) -> Result<Vec<serde_json::Value>>;
    fn find_symbols(&self, pattern: &str, limit: usize) -> Result<Vec<serde_json::Value>>;
    fn symbol_callers(&self, node_id: &str) -> Result<Vec<serde_json::Value>>;
    fn symbol_callees(&self, node_id: &str) -> Result<Vec<serde_json::Value>>;
    fn symbol_members(&self, node_id: &str) -> Result<Vec<serde_json::Value>>;

    /// Execute a backend-native query (SQL for SQLite, Cypher for FalkorDB).
    fn run_raw_query(&self, query: &str) -> Result<Vec<serde_json::Value>>;

    /// Human-readable backend name for logging/UI.
    fn backend_name(&self) -> &'static str;
}

/// Which backend to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Sqlite,
    #[cfg(feature = "falkor")]
    Falkor,
}

/// Default SQLite database path: `.ast-graph/graph.db` inside project root.
pub fn default_db_path(project_root: &Path) -> PathBuf {
    let dir = project_root.join(".ast-graph");
    std::fs::create_dir_all(&dir).ok();
    dir.join("graph.db")
}

/// Open a SQLite-backed storage at the given path.
pub fn open_sqlite(db_path: &Path) -> Result<Box<dyn GraphStorage>> {
    Ok(Box::new(SqliteStorage::open(db_path)?))
}

/// Open an in-memory SQLite store (testing/ephemeral).
pub fn open_sqlite_memory() -> Result<Box<dyn GraphStorage>> {
    Ok(Box::new(SqliteStorage::open_memory()?))
}

/// Connect to a FalkorDB instance.
#[cfg(feature = "falkor")]
pub fn open_falkor(cfg: FalkorConfig) -> Result<Box<dyn GraphStorage>> {
    Ok(Box::new(FalkorStorage::connect(cfg)?))
}
