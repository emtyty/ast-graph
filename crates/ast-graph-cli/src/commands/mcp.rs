//! Minimal MCP (Model Context Protocol) server over stdio.
//!
//! Speaks JSON-RPC 2.0 with newline-delimited messages — the simplest MCP
//! transport. Exposes ast-graph's existing tools (symbol lookup, hotspots,
//! call/blast/dead-code, route/process listings, FTS search, raw query) so
//! Claude Code / Cursor / Codex can call ast-graph natively.
//!
//! This handler is deliberately tiny — no async runtime, no MCP SDK. The
//! `tools/list` payload is built from a static array so the binary stays small.

use anyhow::Result;
use ast_graph_storage::GraphStorage;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use tracing::warn;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "ast-graph";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn run(storage: &dyn GraphStorage, repo_root: &Path) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut reader = BufReader::new(stdin.lock());

    loop {
        let msg = match read_message(&mut reader)? {
            Some(m) => m,
            None => return Ok(()),
        };
        let response = handle(&msg, storage, repo_root);
        if let Some(resp) = response {
            let serialized = serde_json::to_string(&resp)?;
            // Newline-delimited framing — simplest viable MCP transport.
            writeln!(stdout, "{}", serialized)?;
            stdout.flush()?;
        }
    }
}

/// Read a single JSON-RPC message. Supports both newline-delimited messages
/// and Content-Length-framed messages (the LSP-style framing some MCP
/// implementations use).
fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<Value>> {
    // Peek the first byte. If it's `{`, we're in newline-delimited mode.
    // If it's `C` (Content-Length), we use header framing.
    let mut line = String::new();
    let n = reader.read_line(&mut line)?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = line.trim_start();
    if trimmed.starts_with('{') {
        return Ok(Some(serde_json::from_str(line.trim())?));
    }
    if trimmed.to_ascii_lowercase().starts_with("content-length:") {
        let len: usize = trimmed[15..].trim().parse().unwrap_or(0);
        // Consume blank line(s) until empty.
        loop {
            let mut hdr = String::new();
            let m = reader.read_line(&mut hdr)?;
            if m == 0 || hdr.trim().is_empty() {
                break;
            }
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf)?;
        let parsed: Value = serde_json::from_slice(&buf)?;
        return Ok(Some(parsed));
    }
    // Skip unknown / blank lines.
    Ok(Some(json!({"jsonrpc":"2.0","method":"_skip"})))
}

fn handle(msg: &Value, storage: &dyn GraphStorage, repo_root: &Path) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(|v| v.as_str()).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(json!({}));

    match method {
        "_skip" => None,
        "initialize" => Some(reply(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": { "listChanged": false },
                },
                "serverInfo": {
                    "name": SERVER_NAME,
                    "version": SERVER_VERSION,
                }
            }),
        )),
        "notifications/initialized" => None,
        "ping" => Some(reply(id, json!({}))),
        "tools/list" => Some(reply(id, json!({ "tools": tool_definitions() }))),
        "tools/call" => Some(handle_tool_call(id, &params, storage, repo_root)),
        // Resource methods — minimal stubs so clients don't error.
        "resources/list" => Some(reply(id, json!({ "resources": [] }))),
        "prompts/list" => Some(reply(id, json!({ "prompts": [] }))),
        _ => Some(error_reply(
            id,
            -32601,
            &format!("Method not found: {}", method),
        )),
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool_def(
            "symbol",
            "Look up a symbol by name (partial match). Returns matches with file/line and optional callers/callees/members.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string", "description": "Symbol name (partial match)"},
                    "callers": {"type": "boolean", "default": false},
                    "callees": {"type": "boolean", "default": false},
                    "members": {"type": "boolean", "default": false},
                    "limit": {"type": "integer", "default": 10}
                },
                "required": ["name"]
            }),
        ),
        tool_def(
            "call_chain",
            "Trace the CALLS edges downstream from a symbol up to N hops.",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "depth": {"type": "integer", "default": 3}
                },
                "required": ["name"]
            }),
        ),
        tool_def(
            "blast_radius",
            "Reverse traversal: who calls this symbol, transitively?",
            json!({
                "type": "object",
                "properties": {
                    "name": {"type": "string"},
                    "depth": {"type": "integer", "default": 2}
                },
                "required": ["name"]
            }),
        ),
        tool_def(
            "hotspots",
            "List the most-connected symbols in the graph.",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "default": 20}
                }
            }),
        ),
        tool_def(
            "dead_code",
            "List functions/methods with no inbound CALLS edges (likely dead).",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "default": 200},
                    "kinds": {"type": "string", "description": "Comma-separated kinds (Function,Method,Constructor)"},
                    "include_all": {"type": "boolean", "default": false}
                }
            }),
        ),
        tool_def(
            "search",
            "Full-text keyword search over symbol names, signatures, and doc comments (BM25).",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "limit": {"type": "integer", "default": 20}
                },
                "required": ["query"]
            }),
        ),
        tool_def(
            "routes",
            "List all extracted HTTP routes (Express, NestJS, FastAPI, Spring, ASP.NET, Axum, Actix, chi/echo).",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "default": 50}
                }
            }),
        ),
        tool_def(
            "processes",
            "List traced execution flows (entry points + their step counts).",
            json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer", "default": 20}
                }
            }),
        ),
        tool_def(
            "stats",
            "Graph summary: node count, edge count, languages, kind breakdown.",
            json!({"type": "object", "properties": {}}),
        ),
        tool_def(
            "query",
            "Run a backend-native query (SQL on SQLite, Cypher on FalkorDB).",
            json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
        ),
    ]
}

fn tool_def(name: &str, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
    })
}

fn handle_tool_call(
    id: Option<Value>,
    params: &Value,
    storage: &dyn GraphStorage,
    _repo_root: &Path,
) -> Value {
    let tool = params
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let result = dispatch(tool, &args, storage);
    match result {
        Ok(v) => reply(
            id,
            json!({
                "content": [
                    {"type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default()}
                ],
                "isError": false,
            }),
        ),
        Err(e) => {
            warn!("MCP tool {} failed: {}", tool, e);
            reply(
                id,
                json!({
                    "content": [{"type":"text","text": e.to_string()}],
                    "isError": true,
                }),
            )
        }
    }
}

fn dispatch(tool: &str, args: &Value, storage: &dyn GraphStorage) -> Result<Value> {
    match tool {
        "symbol" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let want_callers = args.get("callers").and_then(|v| v.as_bool()).unwrap_or(false);
            let want_callees = args.get("callees").and_then(|v| v.as_bool()).unwrap_or(false);
            let want_members = args.get("members").and_then(|v| v.as_bool()).unwrap_or(false);
            let matches = storage.find_symbols(name, limit)?;
            // Decorate each match with the requested expansions.
            let mut out = Vec::with_capacity(matches.len());
            for m in matches {
                let mut o = m.clone();
                if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                    if want_callers {
                        o["callers"] = json!(storage.symbol_callers(id)?);
                    }
                    if want_callees {
                        o["callees"] = json!(storage.symbol_callees(id)?);
                    }
                    if want_members {
                        o["members"] = json!(storage.symbol_members(id)?);
                    }
                }
                out.push(o);
            }
            Ok(json!(out))
        }
        "call_chain" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let depth = args.get("depth").and_then(|v| v.as_i64()).unwrap_or(3) as i32;
            let candidates = storage.find_symbols(name, 1)?;
            let id = candidates
                .first()
                .and_then(|c| c.get("id"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("symbol not found: {}", name))?;
            Ok(json!(storage.call_chain(id, depth)?))
        }
        "blast_radius" => {
            let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let depth = args.get("depth").and_then(|v| v.as_i64()).unwrap_or(2) as i32;
            let candidates = storage.find_symbols(name, 1)?;
            let id = candidates
                .first()
                .and_then(|c| c.get("id"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("symbol not found: {}", name))?;
            Ok(json!(storage.reverse_call_chain(id, depth)?))
        }
        "hotspots" => {
            let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(20) as i32;
            Ok(json!(storage.hotspots(limit)?))
        }
        "dead_code" => {
            let limit = args.get("limit").and_then(|v| v.as_i64()).unwrap_or(200) as i32;
            let kinds_str = args
                .get("kinds")
                .and_then(|v| v.as_str())
                .unwrap_or("Function,Method,Constructor");
            let kinds: Vec<&str> = kinds_str.split(',').map(|s| s.trim()).collect();
            let include_all = args
                .get("include_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let exclude: &[&str] = if include_all {
                &[]
            } else {
                &[
                    "node_modules",
                    "dist/",
                    ".min.js",
                    "vendor",
                    "target/",
                    "build/",
                    ".git",
                ]
            };
            Ok(json!(storage.dead_symbols(&kinds, exclude, limit)?))
        }
        "search" => {
            let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
            Ok(json!(storage.search_symbols(q, limit)?))
        }
        "routes" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50);
            let q = if storage.backend_name() == "sqlite" {
                format!(
                    "SELECT n.id, n.name, n.file_path, n.line_start,
                            COUNT(DISTINCT e.source_id) AS handlers
                     FROM nodes n
                     LEFT JOIN edges e ON e.target_id = n.id AND e.kind = 'HANDLES_ROUTE'
                     WHERE n.kind = 'Route'
                     GROUP BY n.id ORDER BY n.name LIMIT {limit}"
                )
            } else {
                format!(
                    "MATCH (r:Symbol {{kind:'Route'}}) \
                     OPTIONAL MATCH (h:Symbol)-[:HANDLES_ROUTE]->(r) \
                     WITH r, count(DISTINCT h) AS handlers \
                     RETURN r.id, r.name, r.file_path, r.line_start, handlers \
                     ORDER BY r.name LIMIT {limit}"
                )
            };
            Ok(json!(storage.run_raw_query(&q)?))
        }
        "processes" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
            let q = if storage.backend_name() == "sqlite" {
                format!(
                    "SELECT p.id, p.name, p.file_path, p.line_start,
                            (SELECT COUNT(*) FROM edges e
                             WHERE e.target_id = p.id AND e.kind = 'STEP_IN_PROCESS') AS steps
                     FROM nodes p WHERE p.kind = 'Process'
                     ORDER BY steps DESC LIMIT {limit}"
                )
            } else {
                format!(
                    "MATCH (p:Symbol {{kind:'Process'}}) \
                     OPTIONAL MATCH (s)-[:STEP_IN_PROCESS]->(p) \
                     WITH p, count(s) AS steps \
                     RETURN p.id, p.name, p.file_path, p.line_start, steps \
                     ORDER BY steps DESC LIMIT {limit}"
                )
            };
            Ok(json!(storage.run_raw_query(&q)?))
        }
        "stats" => Ok(storage.get_stats()?),
        "query" => {
            let q = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
            Ok(json!(storage.run_raw_query(q)?))
        }
        other => Err(anyhow::anyhow!("unknown tool: {}", other)),
    }
}

fn reply(id: Option<Value>, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result,
    })
}

fn error_reply(id: Option<Value>, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": code,
            "message": message,
        }
    })
}
