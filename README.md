# ast-graph

A fast, self-contained codebase visualizer that compresses source code into an interactive graph. Uses **tree-sitter** for multi-language AST parsing, **SQLite** for storage, and **Cytoscape.js** for browser-based visualization.

Parse any codebase → extract only the structural skeleton (functions, classes, imports, calls) → store relationships in SQLite → explore as an interactive graph.

## Features

- **Multi-language** — Rust, Python, JavaScript/TypeScript, C# (.NET)
- **AST compression** — strips full syntax trees down to ~10% structural nodes only
- **Cross-file resolution** — resolves function calls, imports, type references, inheritance across files
- **Interactive graph UI** — force-directed layout with community coloring, hover highlighting, click-to-expand
- **Progressive loading** — starts from entry points, double-click to explore deeper
- **SQL queries** — run arbitrary SQL against the graph database
- **AI context export** — compact skeleton format for feeding into LLMs
- **Zero dependencies** — no Docker, no external databases, just a single binary + SQLite file
- **Incremental** — only re-parses changed files on re-scan

## Quick Start

```bash
# Build
cargo build --release

# Scan a project
./target/release/ast-graph scan /path/to/your/project

# Launch web UI
./target/release/ast-graph serve --port 8080 --open
```

Open `http://localhost:8080` — you'll see your codebase as an interactive graph.

## Screenshots

The web UI renders your codebase as a force-directed graph:
- Nodes = symbols (functions, classes, structs, traits, interfaces, enums)
- Node size = number of connections (hub nodes are larger)
- Node color = directory/module group
- Edges = relationships (calls, imports, extends, implements)
- Hover a node to highlight its neighborhood
- Double-click to expand and load more connections
- Right panel: groups, filters, node detail, SQL console

## CLI Commands

```
ast-graph scan <path>              Scan a directory and build the code graph
ast-graph serve [--port 8080]      Start the web UI server
ast-graph export --format <fmt>    Export graph (json, dot, ai-context)
ast-graph stats                    Show graph statistics
ast-graph hotspots [--limit 20]    Most connected symbols (architectural hotspots)
ast-graph call-chain <name>        Trace call chain from a function
ast-graph query "<sql>"            Run a SQL query against the graph DB
```

### Examples

```bash
# Scan and launch UI
ast-graph scan ./my-project --clean
ast-graph serve --port 3000 --open

# Find architectural hotspots
ast-graph hotspots --limit 10

# Trace what a function calls (3 levels deep)
ast-graph call-chain main --depth 3

# Export compact skeleton for AI/LLM context
ast-graph export --format ai-context --max-tokens 4000

# Export DOT format for Graphviz
ast-graph export --format dot --output graph.dot

# Run SQL queries
ast-graph query "SELECT name, kind FROM nodes WHERE kind = 'Class' ORDER BY name"
ast-graph query "SELECT n.name, COUNT(e.target_id) as calls
                 FROM nodes n JOIN edges e ON e.source_id = n.id
                 WHERE e.kind = 'CALLS'
                 GROUP BY n.id ORDER BY calls DESC LIMIT 10"
```

## Web UI

The graph view provides:

| Feature | How |
|---|---|
| **Explore** | Double-click a node to expand its connections |
| **Hover** | Hover to highlight a node's neighborhood |
| **Search** | `Ctrl+K` to search symbols by name |
| **Filter by kind** | Dropdown: Function, Class, Struct, Trait, etc. |
| **Filter by language** | Dropdown: Rust, Python, JS, TS, C# |
| **Toggle edge types** | Checkboxes: Calls, Imports, Extends, etc. |
| **Groups** | Click a group in the sidebar to toggle visibility |
| **Node detail** | Click a node to see signature, file, edges |
| **SQL console** | Run queries directly in the browser |
| **Export PNG** | Click PNG button to download a screenshot |

## AI Context Export

The `ai-context` format produces a compact codebase skeleton for LLM consumption:

```
# Project (rust, typescript) — 494 symbols, 1644 relationships

## src/parser.rs
  pub fn parse_file(path: &Path) -> Result<ParsedFile>  [L12-L45]
    calls: extract_symbols, resolve_imports
    called_by: scan_directory
  pub struct ParsedFile { symbols: Vec<Symbol> }  [L47-L50]

## src/graph.rs
  pub fn build_graph(files: Vec<ParsedFile>) -> CodeGraph  [L22-L60]
    calls: merge_symbols, resolve_cross_file
    called_by: cli::scan::run
```

Use `--max-tokens` to fit within LLM context windows.

## Architecture

```
ast-graph/
  crates/
    ast-graph-core/       Core types: SymbolNode, Edge, CodeGraph, NodeId
    ast-graph-parse/      tree-sitter parsing + per-language extractors
    ast-graph-resolve/    Cross-file import/call/type resolution
    ast-graph-storage/    SQLite persistence + graph queries (recursive CTEs)
    ast-graph-server/     axum HTTP server + embedded SPA (rust-embed)
    ast-graph-web/        TypeScript SPA (Cytoscape.js + esbuild)
    ast-graph-cli/        CLI binary (clap)
```

### Processing Pipeline

```
Source Files
    │
    ▼
tree-sitter Parse (parallel, per-file)
    │
    ▼
AST Compress (keep only structural nodes: fn, class, import, call)
    │
    ▼
Build Graph (nodes = symbols, edges = relationships)
    │
    ▼
Cross-file Resolve (match call targets, imports, types across files)
    │
    ▼
Store in SQLite → Serve Web UI / Export
```

### SQLite Schema

```sql
-- Nodes: every symbol in the codebase
CREATE TABLE nodes (
    id          TEXT PRIMARY KEY,   -- stable hash of (file, name, kind, line)
    name        TEXT NOT NULL,
    kind        TEXT NOT NULL,      -- Function, Class, Struct, Trait, etc.
    file_path   TEXT NOT NULL,
    line_start  INTEGER NOT NULL,
    line_end    INTEGER NOT NULL,
    signature   TEXT,               -- e.g. "pub fn parse(path: &Path) -> Result<T>"
    visibility  TEXT NOT NULL,
    language    TEXT NOT NULL
);

-- Edges: relationships between symbols
CREATE TABLE edges (
    source_id   TEXT NOT NULL,
    target_id   TEXT NOT NULL,
    kind        TEXT NOT NULL       -- CALLS, IMPORTS, EXTENDS, IMPLEMENTS, CONTAINS, REFERENCES
);
```

Graph traversal uses SQLite recursive CTEs:

```sql
-- Call chain: what does main() call, 3 levels deep?
WITH RECURSIVE call_tree(id, name, depth) AS (
    SELECT id, name, 0 FROM nodes WHERE name = 'main'
    UNION ALL
    SELECT n.id, n.name, ct.depth + 1
    FROM call_tree ct
    JOIN edges e ON e.source_id = ct.id AND e.kind = 'CALLS'
    JOIN nodes n ON n.id = e.target_id
    WHERE ct.depth < 3
)
SELECT * FROM call_tree WHERE depth > 0;
```

## Languages Supported

| Language | Extensions | What's Extracted |
|---|---|---|
| **Rust** | `.rs` | fn, struct, enum, trait, impl, use, mod, const |
| **Python** | `.py` | def, class, import, from...import, decorators |
| **JavaScript/TypeScript** | `.js/.ts/.tsx` | function, class, arrow fn, import, interface, enum |
| **C# (.NET)** | `.cs` | class, method, interface, using, namespace, record, enum |

## Building from Source

### Prerequisites

- Rust toolchain (1.70+)
- Node.js (18+) — only needed to rebuild the web UI

### Build

```bash
# Build the Rust binary
cargo build --release

# (Optional) Rebuild the web UI
cd crates/ast-graph-web
npm install
npm run build
cp dist/app.js ../ast-graph-server/static/
cp dist/app.js.map ../ast-graph-server/static/
cd ../..
cargo build --release
```

The web UI assets are embedded in the binary via `rust-embed` — no separate web server needed.

## License

MIT
