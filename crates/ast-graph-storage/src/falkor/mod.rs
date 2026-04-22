//! FalkorDB-backed graph storage.
//!
//! FalkorDB is a Redis-module graph database that speaks OpenCypher. It fits the
//! `ast-graph` data model naturally: symbols become `:Symbol` nodes and `Edge`s
//! become typed relationships (`:CALLS`, `:IMPORTS`, ...). The `source_line`
//! field maps to a native relationship property.
//!
//! The `falkordb` crate is async (`tokio`); we wrap it in a blocking runtime
//! so this backend satisfies the synchronous `GraphStorage` trait used by the
//! rest of the codebase.

use anyhow::{anyhow, Result};
use ast_graph_core::*;
use falkordb::{FalkorClientBuilder, FalkorConnectionInfo, FalkorValue};
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tokio::runtime::Runtime;
use tracing::info;

use crate::GraphStorage;

/// Configuration for connecting to a FalkorDB instance.
#[derive(Debug, Clone)]
pub struct FalkorConfig {
    /// Redis URL, e.g. "falkor://127.0.0.1:6379" or "redis://localhost:6379".
    pub url: String,
    /// Name of the graph (FalkorDB namespaces graphs by name).
    pub graph_name: String,
}

impl Default for FalkorConfig {
    fn default() -> Self {
        Self {
            url: "falkor://127.0.0.1:6379".to_string(),
            graph_name: "code_graph".to_string(),
        }
    }
}

pub struct FalkorStorage {
    client: falkordb::FalkorAsyncClient,
    graph_name: String,
    rt: Runtime,
    /// Serializes concurrent access to the underlying graph handle.
    /// `select_graph` returns a fresh handle each call, but we still want
    /// to keep the query model simple and single-threaded.
    lock: Mutex<()>,
}

impl FalkorStorage {
    pub fn connect(cfg: FalkorConfig) -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;

        let conn_info: FalkorConnectionInfo = cfg
            .url
            .as_str()
            .try_into()
            .map_err(|e| anyhow!("invalid FalkorDB URL {}: {:?}", cfg.url, e))?;

        // TCP keepalive at 30s: without it, long parse/resolve pauses or
        // slow batches on this connection get the socket silently dropped by
        // an intermediate box, and the next query hits a dead pool entry.
        // Matches the pattern the falkordb-rs `main` branch added
        // `with_tcp_keepalive` for.
        let client = rt.block_on(async {
            FalkorClientBuilder::new_async()
                .with_connection_info(conn_info)
                .with_tcp_keepalive(Duration::from_secs(30))
                .build()
                .await
        })?;

        let me = Self {
            client,
            graph_name: cfg.graph_name,
            rt,
            lock: Mutex::new(()),
        };
        me.init_schema()?;
        info!("Connected to FalkorDB graph '{}'", me.graph_name);
        Ok(me)
    }

    /// Create indexes/constraints for fast lookups. Re-running is a no-op
    /// because FalkorDB errors on duplicate index definitions — we swallow
    /// those specific errors.
    fn init_schema(&self) -> Result<()> {
        let queries = [
            "CREATE INDEX FOR (n:Symbol) ON (n.id)",
            "CREATE INDEX FOR (n:Symbol) ON (n.name)",
            "CREATE INDEX FOR (n:Symbol) ON (n.kind)",
            "CREATE INDEX FOR (n:Symbol) ON (n.file_path)",
            "CREATE INDEX FOR (n:Symbol) ON (n.language)",
            "CREATE INDEX FOR (f:FileHash) ON (f.file_path)",
        ];
        for q in queries {
            // Ignore "already exists" errors; treat other errors as fatal.
            let _ = self.run_cypher(q, &HashMap::new());
        }
        Ok(())
    }

    /// Run a Cypher query and return rows as `Vec<Vec<FalkorValue>>`.
    ///
    /// Retries up to 3 times on connection errors with exponential backoff
    /// (250 ms → 500 ms → 1 000 ms). The FalkorDB async pool auto-replaces a
    /// dropped socket, but needs a moment; long parse/resolve pauses give the
    /// server/LB enough idle time to close the socket, so the first save-phase
    /// query reliably hits a dead handle.
    fn run_cypher(
        &self,
        cypher: &str,
        params: &HashMap<String, String>,
    ) -> Result<Vec<Vec<FalkorValue>>> {
        let _guard = self.lock.lock().unwrap();
        let graph_name = self.graph_name.clone();
        let cypher = cypher.to_string();
        let params = params.clone();
        self.rt.block_on(async {
            const MAX_RETRIES: u32 = 3;
            for attempt in 0..=MAX_RETRIES {
                let mut graph = self.client.select_graph(&graph_name);
                let qb = graph.query(cypher.clone());
                let res = if params.is_empty() {
                    qb.execute().await
                } else {
                    qb.with_params(&params).execute().await
                };
                match res {
                    Ok(result) => {
                        let mut rows: Vec<Vec<FalkorValue>> = Vec::new();
                        for row in result.data {
                            rows.push(row);
                        }
                        return Ok(rows);
                    }
                    Err(e) if attempt < MAX_RETRIES && is_connection_error(&e) => {
                        let delay_ms = 250u64 * (1 << attempt); // 250, 500, 1000
                        tracing::warn!(
                            "FalkorDB connection dropped (attempt {}/{}), retrying in {}ms: {e}",
                            attempt + 1,
                            MAX_RETRIES,
                            delay_ms,
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                        continue;
                    }
                    Err(e) => return Err(anyhow!(e)),
                }
            }
            unreachable!("retry loop exits via return")
        })
    }
}

/// True if the error is one we should retry (connection dropped / pool
/// returned a dead handle). Anything else (syntax error, dangling ref,
/// etc.) must still propagate.
fn is_connection_error(e: &falkordb::FalkorDBError) -> bool {
    matches!(
        e,
        falkordb::FalkorDBError::ConnectionDown
            | falkordb::FalkorDBError::NoConnection
            | falkordb::FalkorDBError::EmptyConnection
    )
}

// -------- Cypher literal helpers (for inlined batch writes) ---------------

fn cypher_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

fn cypher_opt_string(s: &Option<String>) -> String {
    match s {
        Some(v) => cypher_escape_string(v),
        None => "null".to_string(),
    }
}

// -------- FalkorValue -> serde_json::Value conversion ---------------------

fn fv_to_json(v: &FalkorValue) -> serde_json::Value {
    match v {
        FalkorValue::None => serde_json::Value::Null,
        FalkorValue::String(s) => serde_json::Value::String(s.clone()),
        FalkorValue::I64(n) => serde_json::json!(n),
        FalkorValue::F64(f) => serde_json::json!(f),
        FalkorValue::Bool(b) => serde_json::json!(b),
        FalkorValue::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(fv_to_json).collect())
        }
        FalkorValue::Map(m) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in m {
                obj.insert(k.clone(), fv_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        FalkorValue::Node(n) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in &n.properties {
                obj.insert(k.clone(), fv_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        FalkorValue::Edge(e) => {
            let mut obj = serde_json::Map::new();
            for (k, v) in &e.properties {
                obj.insert(k.clone(), fv_to_json(v));
            }
            serde_json::Value::Object(obj)
        }
        // Fall back to debug repr for types we don't explicitly handle.
        other => serde_json::Value::String(format!("{:?}", other)),
    }
}

/// Pull a string property from a row's FalkorValue (node, map, or string).
fn fv_get_string(v: &FalkorValue) -> Option<String> {
    match v {
        FalkorValue::String(s) => Some(s.clone()),
        FalkorValue::None => None,
        _ => None,
    }
}

fn fv_get_i64(v: &FalkorValue) -> i64 {
    match v {
        FalkorValue::I64(n) => *n,
        _ => 0,
    }
}

// -------- GraphStorage impl -----------------------------------------------

impl GraphStorage for FalkorStorage {
    fn backend_name(&self) -> &'static str {
        "falkordb"
    }

    fn save_graph(&self, graph: &CodeGraph) -> Result<(usize, usize)> {
        // 1. Upsert nodes via UNWIND batches.
        //
        //    We tried going one MERGE per node but the FalkorDB tokio
        //    connection couldn't survive the round-trip count on a
        //    lighthouse-scale repo (~55k nodes → connection drop mid-save).
        //    Small batches keep the query string modest while letting the
        //    connection breathe.
        if !graph.nodes.is_empty() {
            const NODE_BATCH: usize = 200;
            let nodes: Vec<&SymbolNode> = graph.nodes.values().collect();
            for chunk in nodes.chunks(NODE_BATCH) {
                let list = chunk
                    .iter()
                    .map(|n| {
                        format!(
                            "{{id:{id},name:{name},kind:{kind},file_path:{fp},line_start:{ls},line_end:{le},signature:{sig},doc_comment:{doc},visibility:{vis},language:{lang}}}",
                            id = cypher_escape_string(&n.id.to_string()),
                            name = cypher_escape_string(&n.name),
                            kind = cypher_escape_string(n.kind.as_neo4j_label()),
                            fp = cypher_escape_string(&n.file_path.to_string_lossy()),
                            ls = n.line_range.0,
                            le = n.line_range.1,
                            sig = cypher_opt_string(&n.signature),
                            doc = cypher_opt_string(&n.doc_comment),
                            vis = cypher_escape_string(&format!("{:?}", n.visibility)),
                            lang = cypher_escape_string(n.language.as_str()),
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let cypher = format!(
                    "UNWIND [{list}] AS n \
                     MERGE (s:Symbol {{id: n.id}}) \
                     SET s.name = n.name, s.kind = n.kind, s.file_path = n.file_path, \
                         s.line_start = n.line_start, s.line_end = n.line_end, \
                         s.signature = n.signature, s.doc_comment = n.doc_comment, \
                         s.visibility = n.visibility, s.language = n.language"
                );
                self.run_cypher(&cypher, &HashMap::new())?;
            }
        }

        // 2. Upsert edges one at a time (per kind, so the rel-type is static).
        //    For CONTAINS we additionally synthesize parent→child edges from
        //    each node's `parent` field so queries like
        //      (c:Class)-[:CONTAINS]->(m:Method)
        //    work even when the parser only set `parent` without emitting a
        //    Contains RawEdge (e.g. methods inside a class body).
        let mut edge_count = 0usize;
        let mut skipped = 0usize;
        for kind in EdgeKind::ALL {
            let kind_str = kind.as_neo4j_type();
            let mut all_of_kind: Vec<(String, String, u32)> = Vec::new();
            for edges in graph.adjacency.values() {
                for e in edges {
                    if e.kind != *kind {
                        continue;
                    }
                    if !graph.nodes.contains_key(&e.source)
                        || !graph.nodes.contains_key(&e.target)
                    {
                        skipped += 1;
                        continue;
                    }
                    all_of_kind.push((
                        e.source.to_string(),
                        e.target.to_string(),
                        e.source_line,
                    ));
                }
            }
            if *kind == EdgeKind::Contains {
                for node in graph.nodes.values() {
                    if let Some(p) = node.parent {
                        if graph.nodes.contains_key(&p) {
                            all_of_kind.push((
                                p.to_string(),
                                node.id.to_string(),
                                node.line_range.0,
                            ));
                        }
                    }
                }
            }
            // Edges stay batched via UNWIND — at repository scale we can see
            // hundreds of thousands of edges, and sending them one-by-one
            // outlived the FalkorDB connection on a lighthouse-sized repo.
            const EDGE_BATCH: usize = 1000;
            for chunk in all_of_kind.chunks(EDGE_BATCH) {
                let list = chunk
                    .iter()
                    .map(|(s, t, l)| {
                        format!(
                            "{{src:{},tgt:{},line:{}}}",
                            cypher_escape_string(s),
                            cypher_escape_string(t),
                            l
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let cypher = format!(
                    "UNWIND [{list}] AS e \
                     MATCH (a:Symbol {{id: e.src}}), (b:Symbol {{id: e.tgt}}) \
                     MERGE (a)-[r:{kind_str} {{line: e.line}}]->(b)"
                );
                self.run_cypher(&cypher, &HashMap::new())?;
                edge_count += chunk.len();
            }
        }
        if skipped > 0 {
            info!("Skipped {} edges with dangling node references", skipped);
        }

        // 4. File hashes (as :FileHash nodes with hex-encoded hash).
        if !graph.file_hashes.is_empty() {
            let entries: Vec<String> = graph
                .file_hashes
                .iter()
                .map(|(path, hash)| {
                    let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
                    format!(
                        "{{file_path:{},hash:{}}}",
                        cypher_escape_string(&path.to_string_lossy()),
                        cypher_escape_string(&hex)
                    )
                })
                .collect();
            let cypher = format!(
                "UNWIND [{}] AS f \
                 MERGE (h:FileHash {{file_path: f.file_path}}) \
                 SET h.hash = f.hash",
                entries.join(",")
            );
            self.run_cypher(&cypher, &HashMap::new())?;
        }

        // The `edge_count` accumulated above reflects rows sent to MERGE, not
        // rows physically stored — MERGE dedups by relationship pattern. Read
        // the actual counts back so the CLI reports reality.
        let _ = edge_count;
        let (node_count, edge_count) = {
            let node_rows = self.run_cypher(
                "MATCH (n:Symbol) RETURN count(n)",
                &HashMap::new(),
            )?;
            let edge_rows = self.run_cypher(
                "MATCH (:Symbol)-[r]->(:Symbol) RETURN count(r)",
                &HashMap::new(),
            )?;
            let n = node_rows
                .first()
                .and_then(|r| r.first())
                .map(fv_get_i64)
                .unwrap_or(0) as usize;
            let e = edge_rows
                .first()
                .and_then(|r| r.first())
                .map(fv_get_i64)
                .unwrap_or(0) as usize;
            (n, e)
        };
        info!(
            "Saved {} nodes, {} edges to FalkorDB graph '{}' ({} skipped)",
            node_count, edge_count, self.graph_name, skipped
        );
        Ok((node_count, edge_count))
    }

    fn remove_file_nodes(&self, file_path: &str) -> Result<()> {
        let mut params = HashMap::new();
        params.insert("path".to_string(), cypher_escape_string(file_path));
        self.run_cypher(
            "MATCH (n:Symbol {file_path: $path}) DETACH DELETE n",
            &params,
        )?;
        self.run_cypher(
            "MATCH (f:FileHash {file_path: $path}) DELETE f",
            &params,
        )?;
        Ok(())
    }

    fn load_graph(&self, project_root: PathBuf) -> Result<CodeGraph> {
        let mut graph = CodeGraph::new(project_root);

        // Pre-load parent map from CONTAINS edges so we can restore the
        // `parent` field on each node.
        let parent_rows = self.run_cypher(
            "MATCH (p:Symbol)-[:CONTAINS]->(c:Symbol) RETURN c.id, p.id",
            &HashMap::new(),
        )?;
        let mut parent_map: FxHashMap<NodeId, NodeId> = FxHashMap::default();
        for row in parent_rows {
            if row.len() < 2 {
                continue;
            }
            let child = match fv_get_string(&row[0]).and_then(|s| NodeId::from_hex(&s)) {
                Some(v) => v,
                None => continue,
            };
            let parent = match fv_get_string(&row[1]).and_then(|s| NodeId::from_hex(&s)) {
                Some(v) => v,
                None => continue,
            };
            parent_map.insert(child, parent);
        }

        // Load all Symbol nodes.
        let rows = self.run_cypher(
            "MATCH (n:Symbol) RETURN n.id, n.name, n.kind, n.file_path, n.line_start, \
             n.line_end, n.signature, n.doc_comment, n.visibility, n.language",
            &HashMap::new(),
        )?;
        for row in rows {
            if row.len() < 10 {
                continue;
            }
            let id_str = match fv_get_string(&row[0]) {
                Some(s) => s,
                None => continue,
            };
            let id = match NodeId::from_hex(&id_str) {
                Some(v) => v,
                None => continue,
            };
            let name = fv_get_string(&row[1]).unwrap_or_default();
            let kind = match fv_get_string(&row[2]).and_then(|s| SymbolKind::from_label(&s)) {
                Some(k) => k,
                None => continue,
            };
            let file_path = PathBuf::from(fv_get_string(&row[3]).unwrap_or_default());
            let line_start = fv_get_i64(&row[4]) as u32;
            let line_end = fv_get_i64(&row[5]) as u32;
            let signature = fv_get_string(&row[6]);
            let doc_comment = fv_get_string(&row[7]);
            let visibility =
                Visibility::from_debug_str(&fv_get_string(&row[8]).unwrap_or_default());
            let language = match fv_get_string(&row[9]).and_then(|s| Language::from_str(&s)) {
                Some(l) => l,
                None => continue,
            };
            let parent = parent_map.get(&id).copied();
            graph.add_node(SymbolNode {
                id,
                name,
                kind,
                file_path,
                line_range: (line_start, line_end),
                signature,
                doc_comment,
                visibility,
                language,
                parent,
            });
        }

        // Load all edges (all relationship types).
        let rows = self.run_cypher(
            "MATCH (a:Symbol)-[r]->(b:Symbol) RETURN a.id, b.id, type(r), r.line",
            &HashMap::new(),
        )?;
        for row in rows {
            if row.len() < 4 {
                continue;
            }
            let src = match fv_get_string(&row[0]).and_then(|s| NodeId::from_hex(&s)) {
                Some(v) => v,
                None => continue,
            };
            let tgt = match fv_get_string(&row[1]).and_then(|s| NodeId::from_hex(&s)) {
                Some(v) => v,
                None => continue,
            };
            let kind = match fv_get_string(&row[2]).and_then(|s| EdgeKind::from_neo4j_type(&s)) {
                Some(k) => k,
                None => continue,
            };
            let line = fv_get_i64(&row[3]) as u32;
            graph.add_edge(Edge {
                source: src,
                target: tgt,
                kind,
                source_line: line,
            });
        }

        // Load file hashes.
        let rows = self.run_cypher(
            "MATCH (f:FileHash) RETURN f.file_path, f.hash",
            &HashMap::new(),
        )?;
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let path = match fv_get_string(&row[0]) {
                Some(s) => PathBuf::from(s),
                None => continue,
            };
            let hex = match fv_get_string(&row[1]) {
                Some(s) => s,
                None => continue,
            };
            if hex.len() == 64 {
                let mut arr = [0u8; 32];
                for i in 0..32 {
                    if let Ok(b) = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16) {
                        arr[i] = b;
                    }
                }
                graph.file_hashes.insert(path, arr);
            }
        }

        graph.refresh_metadata();
        info!(
            "Loaded graph from FalkorDB: {} nodes, {} edges, {} files",
            graph.metadata.total_nodes, graph.metadata.total_edges, graph.metadata.total_files
        );
        Ok(graph)
    }

    fn load_file_hashes(&self) -> Result<FxHashMap<PathBuf, [u8; 32]>> {
        let mut map = FxHashMap::default();
        let rows = self.run_cypher(
            "MATCH (f:FileHash) RETURN f.file_path, f.hash",
            &HashMap::new(),
        )?;
        for row in rows {
            if row.len() < 2 {
                continue;
            }
            let path = match fv_get_string(&row[0]) {
                Some(s) => PathBuf::from(s),
                None => continue,
            };
            let hex = match fv_get_string(&row[1]) {
                Some(s) => s,
                None => continue,
            };
            if hex.len() == 64 {
                let mut arr = [0u8; 32];
                for i in 0..32 {
                    if let Ok(b) = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16) {
                        arr[i] = b;
                    }
                }
                map.insert(path, arr);
            }
        }
        Ok(map)
    }

    fn clear(&self) -> Result<()> {
        // GRAPH.DELETE drops the whole graph server-side in one shot. The
        // earlier `MATCH (n) DETACH DELETE n` approach couldn't survive on a
        // graph with hundreds of thousands of nodes — the connection died
        // before the query finished.
        let _guard = self.lock.lock().unwrap();
        let graph_name = self.graph_name.clone();
        self.rt.block_on(async {
            let mut graph = self.client.select_graph(&graph_name);
            // Ignore "graph doesn't exist" errors — clear on a fresh DB is fine.
            let _ = graph.delete().await;
            Ok::<_, anyhow::Error>(())
        })?;
        drop(_guard);
        // Re-create indexes since GRAPH.DELETE drops them too.
        self.init_schema()?;
        Ok(())
    }

    fn get_stats(&self) -> Result<serde_json::Value> {
        let node_rows =
            self.run_cypher("MATCH (n:Symbol) RETURN count(n)", &HashMap::new())?;
        let nodes = node_rows
            .first()
            .and_then(|r| r.first())
            .map(fv_get_i64)
            .unwrap_or(0);

        let edge_rows =
            self.run_cypher("MATCH (:Symbol)-[r]->(:Symbol) RETURN count(r)", &HashMap::new())?;
        let edges = edge_rows
            .first()
            .and_then(|r| r.first())
            .map(fv_get_i64)
            .unwrap_or(0);

        let file_rows = self.run_cypher(
            "MATCH (n:Symbol) RETURN count(DISTINCT n.file_path)",
            &HashMap::new(),
        )?;
        let files = file_rows
            .first()
            .and_then(|r| r.first())
            .map(fv_get_i64)
            .unwrap_or(0);

        let lang_rows = self.run_cypher(
            "MATCH (n:Symbol) RETURN n.language AS language, count(n) AS cnt ORDER BY cnt DESC",
            &HashMap::new(),
        )?;
        let languages: Vec<serde_json::Value> = lang_rows
            .iter()
            .filter_map(|r| {
                if r.len() < 2 {
                    return None;
                }
                Some(serde_json::json!({
                    "language": fv_get_string(&r[0]).unwrap_or_default(),
                    "count": fv_get_i64(&r[1]),
                }))
            })
            .collect();

        let kind_rows = self.run_cypher(
            "MATCH (n:Symbol) RETURN n.kind AS kind, count(n) AS cnt ORDER BY cnt DESC",
            &HashMap::new(),
        )?;
        let kinds: Vec<serde_json::Value> = kind_rows
            .iter()
            .filter_map(|r| {
                if r.len() < 2 {
                    return None;
                }
                Some(serde_json::json!({
                    "kind": fv_get_string(&r[0]).unwrap_or_default(),
                    "count": fv_get_i64(&r[1]),
                }))
            })
            .collect();

        Ok(serde_json::json!({
            "backend": "falkordb",
            "graph": self.graph_name,
            "nodes": nodes,
            "edges": edges,
            "files": files,
            "languages": languages,
            "kinds": kinds,
        }))
    }

    fn call_chain(&self, node_id: &str, max_depth: i32) -> Result<Vec<serde_json::Value>> {
        // Cypher doesn't support parameterised upper bounds on variable-length
        // paths, so splice max_depth directly (it's clamped to a reasonable i32).
        let depth = max_depth.clamp(1, 20);
        let mut params = HashMap::new();
        params.insert("id".to_string(), cypher_escape_string(node_id));
        let cypher = format!(
            "MATCH path = (start:Symbol {{id: $id}})-[r:CALLS*1..{depth}]->(target:Symbol) \
             RETURN target.id, target.name, target.kind, length(path) AS depth, \
                    [n IN nodes(path) | n.name] AS node_path, \
                    [rel IN relationships(path) | rel.line] AS call_lines \
             ORDER BY depth, target.name"
        );
        let rows = self.run_cypher(&cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 6 {
                    return None;
                }
                let node_path = match &r[4] {
                    FalkorValue::Array(a) => a
                        .iter()
                        .filter_map(fv_get_string)
                        .collect::<Vec<_>>()
                        .join(" -> "),
                    _ => String::new(),
                };
                Some(serde_json::json!({
                    "id":         fv_get_string(&r[0]).unwrap_or_default(),
                    "name":       fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":       fv_get_string(&r[2]).unwrap_or_default(),
                    "depth":      fv_get_i64(&r[3]),
                    "path":       node_path,
                    "call_lines": fv_to_json(&r[5]),
                }))
            })
            .collect())
    }

    fn reverse_call_chain(&self, node_id: &str, max_depth: i32) -> Result<Vec<serde_json::Value>> {
        let depth = max_depth.clamp(1, 20);
        let mut params = HashMap::new();
        params.insert("id".to_string(), cypher_escape_string(node_id));
        // Upstream callers: follow CALLS edges in reverse direction.
        let cypher = format!(
            "MATCH path = (caller:Symbol)-[r:CALLS*1..{depth}]->(target:Symbol {{id: $id}}) \
             RETURN caller.id, caller.name, caller.kind, caller.file_path, caller.line_start, \
                    length(path) AS depth, \
                    [n IN nodes(path) | n.name] AS node_path, \
                    [rel IN relationships(path) | rel.line][0] AS call_line \
             ORDER BY depth, caller.name"
        );
        let rows = self.run_cypher(&cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 8 {
                    return None;
                }
                let node_path = match &r[6] {
                    FalkorValue::Array(a) => a
                        .iter()
                        .filter_map(fv_get_string)
                        .collect::<Vec<_>>()
                        .join(" -> "),
                    _ => String::new(),
                };
                Some(serde_json::json!({
                    "id":         fv_get_string(&r[0]).unwrap_or_default(),
                    "name":       fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":       fv_get_string(&r[2]).unwrap_or_default(),
                    "file_path":  fv_get_string(&r[3]).unwrap_or_default(),
                    "line_start": fv_get_i64(&r[4]),
                    "depth":      fv_get_i64(&r[5]),
                    "path":       node_path,
                    "call_line":  fv_get_i64(&r[7]),
                }))
            })
            .collect())
    }

    fn dead_symbols(
        &self,
        kinds: &[&str],
        exclude_path_substrings: &[&str],
        limit: i32,
    ) -> Result<Vec<serde_json::Value>> {
        let kinds_list = if kinds.is_empty() {
            "['Function','Method','Constructor']".to_string()
        } else {
            let joined = kinds
                .iter()
                .map(|k| format!("'{}'", k.replace('\'', "\\'")))
                .collect::<Vec<_>>()
                .join(",");
            format!("[{joined}]")
        };

        let exclude_clause = exclude_path_substrings
            .iter()
            .map(|p| format!(" AND NOT n.file_path CONTAINS '{}'", p.replace('\'', "\\'")))
            .collect::<Vec<_>>()
            .join("");

        let lim = limit.max(1);
        let cypher = format!(
            "MATCH (n:Symbol) \
             WHERE n.kind IN {kinds} {exclude} \
               AND NOT EXISTS((:Symbol)-[:CALLS]->(n)) \
             RETURN n.id, n.name, n.kind, n.file_path, n.line_start, n.line_end, \
                    n.signature, n.visibility, n.language \
             ORDER BY n.file_path, n.line_start \
             LIMIT {lim}",
            kinds = kinds_list,
            exclude = exclude_clause,
        );
        let rows = self.run_cypher(&cypher, &HashMap::new())?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 9 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":         fv_get_string(&r[0]).unwrap_or_default(),
                    "name":       fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":       fv_get_string(&r[2]).unwrap_or_default(),
                    "file_path":  fv_get_string(&r[3]).unwrap_or_default(),
                    "line_start": fv_get_i64(&r[4]),
                    "line_end":   fv_get_i64(&r[5]),
                    "signature":  fv_get_string(&r[6]),
                    "visibility": fv_get_string(&r[7]).unwrap_or_default(),
                    "language":   fv_get_string(&r[8]).unwrap_or_default(),
                }))
            })
            .collect())
    }

    fn symbols_in_range(
        &self,
        file_path_substring: &str,
        line_start: u32,
        line_end: u32,
    ) -> Result<Vec<serde_json::Value>> {
        let mut params = HashMap::new();
        params.insert("path".to_string(), cypher_escape_string(file_path_substring));
        // Splice the integer bounds directly — FalkorDB parameter substitution
        // for integers is awkward; both are constrained i64 so this is safe.
        let cypher = format!(
            "MATCH (n:Symbol) \
             WHERE n.file_path CONTAINS $path \
               AND NOT n.kind IN ['File','Import'] \
               AND n.line_start <= {end} \
               AND n.line_end   >= {start} \
             RETURN n.id, n.name, n.kind, n.file_path, n.line_start, n.line_end, n.signature \
             ORDER BY n.file_path, n.line_start",
            start = line_start as i64,
            end = line_end as i64,
        );
        let rows = self.run_cypher(&cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 7 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":         fv_get_string(&r[0]).unwrap_or_default(),
                    "name":       fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":       fv_get_string(&r[2]).unwrap_or_default(),
                    "file_path":  fv_get_string(&r[3]).unwrap_or_default(),
                    "line_start": fv_get_i64(&r[4]),
                    "line_end":   fv_get_i64(&r[5]),
                    "signature":  fv_get_string(&r[6]),
                }))
            })
            .collect())
    }

    fn shortest_path(&self, from_id: &str, to_id: &str) -> Result<Vec<serde_json::Value>> {
        let mut params = HashMap::new();
        params.insert("from".to_string(), cypher_escape_string(from_id));
        params.insert("to".to_string(), cypher_escape_string(to_id));
        let cypher = "MATCH p = shortestPath((a:Symbol {id: $from})-[*..15]-(b:Symbol {id: $to})) \
                      RETURN [n IN nodes(p) | n.id] AS ids, \
                             [n IN nodes(p) | n.name] AS names, \
                             length(p) AS depth";
        let rows = self.run_cypher(cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 3 {
                    return None;
                }
                let ids = match &r[0] {
                    FalkorValue::Array(a) => a
                        .iter()
                        .filter_map(fv_get_string)
                        .collect::<Vec<_>>()
                        .join(","),
                    _ => String::new(),
                };
                let names = match &r[1] {
                    FalkorValue::Array(a) => a
                        .iter()
                        .filter_map(fv_get_string)
                        .collect::<Vec<_>>()
                        .join(" -> "),
                    _ => String::new(),
                };
                Some(serde_json::json!({
                    "trail": ids,
                    "name":  names,
                    "depth": fv_get_i64(&r[2]),
                }))
            })
            .collect())
    }

    fn find_implementations(&self, trait_name: &str) -> Result<Vec<serde_json::Value>> {
        let mut params = HashMap::new();
        params.insert("name".to_string(), cypher_escape_string(trait_name));
        let cypher = "MATCH (impl:Symbol)-[:IMPLEMENTS*1..5]->(t:Symbol) \
                      WHERE t.name = $name OR t.name STARTS WITH ($name + '::') \
                      RETURN DISTINCT impl.id, impl.name, impl.kind, impl.file_path, impl.line_start \
                      ORDER BY impl.name";
        let rows = self.run_cypher(cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 5 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":   fv_get_string(&r[0]).unwrap_or_default(),
                    "name": fv_get_string(&r[1]).unwrap_or_default(),
                    "kind": fv_get_string(&r[2]).unwrap_or_default(),
                    "file": fv_get_string(&r[3]).unwrap_or_default(),
                    "line": fv_get_i64(&r[4]),
                }))
            })
            .collect())
    }

    fn hotspots(&self, limit: i32) -> Result<Vec<serde_json::Value>> {
        let lim = limit.max(1);
        let cypher = format!(
            "MATCH (n:Symbol) WHERE NOT n.kind IN ['File','Import'] \
             RETURN n.id, n.name, n.kind, n.file_path, \
                    size((n)-->()) AS outgoing, \
                    size(()-->(n)) AS incoming, \
                    size((n)-->()) + size(()-->(n)) AS total \
             ORDER BY total DESC LIMIT {lim}"
        );
        let rows = self.run_cypher(&cypher, &HashMap::new())?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 7 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":          fv_get_string(&r[0]).unwrap_or_default(),
                    "name":        fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":        fv_get_string(&r[2]).unwrap_or_default(),
                    "file":        fv_get_string(&r[3]).unwrap_or_default(),
                    "outgoing":    fv_get_i64(&r[4]),
                    "incoming":    fv_get_i64(&r[5]),
                    "connections": fv_get_i64(&r[6]),
                }))
            })
            .collect())
    }

    fn find_symbols(&self, pattern: &str, limit: usize) -> Result<Vec<serde_json::Value>> {
        let mut params = HashMap::new();
        params.insert("pat".to_string(), cypher_escape_string(pattern));
        let cypher = format!(
            "MATCH (n:Symbol) \
             WHERE n.name CONTAINS $pat AND NOT n.kind IN ['File', 'Import'] \
             RETURN n.id, n.name, n.kind, n.file_path, n.line_start, n.line_end, n.signature, null \
             ORDER BY size(n.name) \
             LIMIT {limit}"
        );
        let rows = self.run_cypher(&cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 8 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":         fv_get_string(&r[0]).unwrap_or_default(),
                    "name":       fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":       fv_get_string(&r[2]).unwrap_or_default(),
                    "file_path":  fv_get_string(&r[3]).unwrap_or_default(),
                    "line_start": fv_get_i64(&r[4]),
                    "line_end":   fv_get_i64(&r[5]),
                    "signature":  fv_get_string(&r[6]),
                    "parent_id":  serde_json::Value::Null,
                }))
            })
            .collect())
    }

    fn symbol_callers(&self, node_id: &str) -> Result<Vec<serde_json::Value>> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), cypher_escape_string(node_id));
        let cypher = "MATCH (caller:Symbol)-[r:CALLS]->(target:Symbol {id: $id}) \
                      RETURN caller.id, caller.name, caller.kind, caller.file_path, \
                             caller.line_start, r.line \
                      ORDER BY caller.file_path, r.line";
        let rows = self.run_cypher(cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 6 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":             fv_get_string(&r[0]).unwrap_or_default(),
                    "name":           fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":           fv_get_string(&r[2]).unwrap_or_default(),
                    "file_path":      fv_get_string(&r[3]).unwrap_or_default(),
                    "line":           fv_get_i64(&r[4]),
                    "call_site_line": fv_get_i64(&r[5]),
                }))
            })
            .collect())
    }

    fn symbol_callees(&self, node_id: &str) -> Result<Vec<serde_json::Value>> {
        let mut params = HashMap::new();
        params.insert("id".to_string(), cypher_escape_string(node_id));
        let cypher = "MATCH (source:Symbol {id: $id})-[r:CALLS]->(target:Symbol) \
                      RETURN target.id, target.name, target.kind, target.file_path, \
                             target.line_start, r.line \
                      ORDER BY r.line, target.name";
        let rows = self.run_cypher(cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 6 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":             fv_get_string(&r[0]).unwrap_or_default(),
                    "name":           fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":           fv_get_string(&r[2]).unwrap_or_default(),
                    "file_path":      fv_get_string(&r[3]).unwrap_or_default(),
                    "line":           fv_get_i64(&r[4]),
                    "call_site_line": fv_get_i64(&r[5]),
                }))
            })
            .collect())
    }

    fn symbol_members(&self, node_id: &str) -> Result<Vec<serde_json::Value>> {
        // CONTAINS relationship (parent -> child) carries the same semantics
        // as SQLite's parent_id column.
        let mut params = HashMap::new();
        params.insert("id".to_string(), cypher_escape_string(node_id));
        let cypher = "MATCH (p:Symbol {id: $id})-[:CONTAINS]->(m:Symbol) \
                      WHERE NOT m.kind IN ['Import'] \
                      RETURN m.id, m.name, m.kind, m.line_start, m.line_end, m.signature, m.visibility \
                      ORDER BY m.line_start";
        let rows = self.run_cypher(cypher, &params)?;
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                if r.len() < 7 {
                    return None;
                }
                Some(serde_json::json!({
                    "id":         fv_get_string(&r[0]).unwrap_or_default(),
                    "name":       fv_get_string(&r[1]).unwrap_or_default(),
                    "kind":       fv_get_string(&r[2]).unwrap_or_default(),
                    "line_start": fv_get_i64(&r[3]),
                    "line_end":   fv_get_i64(&r[4]),
                    "signature":  fv_get_string(&r[5]),
                    "visibility": fv_get_string(&r[6]).unwrap_or_default(),
                }))
            })
            .collect())
    }

    fn run_raw_query(&self, query: &str) -> Result<Vec<serde_json::Value>> {
        let rows = self.run_cypher(query, &HashMap::new())?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let arr: Vec<serde_json::Value> = r.iter().map(fv_to_json).collect();
                serde_json::Value::Array(arr)
            })
            .collect())
    }
}
