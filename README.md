# ast-graph

A fast codebase intelligence tool. Uses **tree-sitter** for multi-language AST parsing and a pluggable storage backend — **SQLite** (zero-setup, single file) or **FalkorDB** (Redis-module graph database, OpenCypher). Parse any codebase → extract its structural skeleton (functions, classes, imports, calls) → query it instantly from the CLI.

```bash
ast-graph scan ./my-project
ast-graph symbol "MyService"
ast-graph hotspots
```

## Features

- **Multi-language** — Rust, Python, JavaScript/TypeScript, C# (.NET), Java
- **AST compression** — strips full syntax trees down to structural nodes only (~90% reduction)
- **Class-context-aware resolution** — `this.method()` / `self.method()` calls resolve to the correct class, not every method with that name across the codebase
- **Cross-file resolution** — resolves function calls, imports, type references, inheritance across files
- **Symbol lookup** — find any symbol by partial name, instantly see callers, callees, members
- **Pluggable backends** — SQLite for single-machine use, FalkorDB for team/server use with Cypher
- **SQL / Cypher escape hatch** — run arbitrary SQL (SQLite) or Cypher (FalkorDB) against the graph
- **AI context export** — compact skeleton format for feeding into LLMs
- **Self-contained default** — SQLite backend needs no Docker or external services; single binary + `.db` file
- **Incremental scan** — only re-parses changed files on re-scan

## Quick Start

```bash
cargo build --release

# Scan a project
./target/release/ast-graph scan /path/to/your/project

# Look up a symbol
./target/release/ast-graph symbol "MyService"

# Find architectural hotspots
./target/release/ast-graph hotspots
```

## CLI Commands

```
ast-graph scan <path>                 Scan a directory and build the code graph
ast-graph symbol <name>               Look up a symbol — callers, callees, members
ast-graph hotspots [--limit 20]       Most connected symbols (architectural hotspots)
ast-graph call-chain <name>           Trace call chain from a function (recursive)
ast-graph query "<sql|cypher>"        Run a backend-native query (SQL for SQLite, Cypher for FalkorDB)
ast-graph stats                       Graph summary: nodes, edges, languages
ast-graph export --format <fmt>       Export graph (json, dot, ai-context)
```

**Backend flags** (global, apply to every subcommand):

```
--backend <sqlite|falkor>             Which storage backend to use (default: sqlite)
--db <path>                           SQLite database path (sqlite only)
--falkor-url <url>                    FalkorDB URL, or env FALKOR_URL (default: falkor://127.0.0.1:6379)
--falkor-graph-name <name>            FalkorDB graph name (default: code_graph)
```

## Symbol Lookup

The `symbol` command is the primary way to explore the graph:

```bash
# Find all nodes matching a partial name
ast-graph symbol "MyService"

# Exact class — shows all members + callers + callees
ast-graph symbol "UserService"

# Specific method — shows callers and callees
ast-graph symbol "UserService.login"

# Focus on one section
ast-graph symbol "OrderComponent" --members
ast-graph symbol "OrderComponent.submit" --callers
ast-graph symbol "OrderComponent.submit" --callees
```

Example output:

```
┌─ UserService.login [Method]
│  File: src/services/user.service.ts L42-78
│  Sig:  login(email: string, password: string): Observable<User>
│
├─ Callers (3):
│  ← LoginComponent.onSubmit @ src/pages/login/login.component.ts L55
│  ← AuthGuard.canActivate @ src/guards/auth.guard.ts L22
│  ← SessionService.restore @ src/services/session.service.ts L18
│
└─ Calls (4):
   → ApiService.post [Method] @ src/core/services/api.service.ts
   → TokenService.store [Method] @ src/services/token.service.ts
   → UserService.setCurrentUser [Method] @ src/services/user.service.ts
   → LogService.info [Method] @ src/services/log.service.ts
```

## Storage Backends

ast-graph stores the graph in one of two backends; the CLI is identical across both.

| Backend | Query language | When to use |
|---|---|---|
| **SQLite** (default) | SQL + recursive CTEs | Single-developer use, CI pipelines, offline analysis. Zero setup — the database is a single `.db` file. |
| **FalkorDB** | OpenCypher | Team/server deployments, richer graph queries (`shortestPath`, variable-length patterns), sharing one graph across many clients. |

### SQLite (default)

```bash
# Implicit sqlite — writes to .ast-graph/graph.db under the scan root
ast-graph scan ./my-project

# Explicit path
ast-graph --db ./analysis.db scan ./my-project
ast-graph --db ./analysis.db symbol MyService
```

### FalkorDB

FalkorDB is a Redis module. Start it with Docker:

```bash
docker run -d -p 6379:6379 falkordb/falkordb:latest
```

Then point ast-graph at it via `--backend falkor`:

```bash
# Scan into FalkorDB (clears the prior graph)
ast-graph --backend falkor scan ./my-project --clean

# All subsequent commands take the same flag
ast-graph --backend falkor symbol MyService
ast-graph --backend falkor hotspots --limit 20
ast-graph --backend falkor call-chain MyService.login --depth 3

# Or set the URL once via env — no need to pass --falkor-url
export FALKOR_URL="falkor://my-host:6379"
ast-graph --backend falkor stats
```

All nodes carry the `:Symbol` label and all relationships expose a `line` property pointing at the call site (0 if no meaningful line).

## SQL Queries (SQLite backend)

```bash
# All classes in a file
ast-graph query "SELECT name, line_start, line_end FROM nodes
                 WHERE file_path LIKE '%team-on-set%' AND kind != 'Import'
                 ORDER BY line_start"

# Most-called methods
ast-graph query "SELECT n.name, COUNT(*) as callers
                 FROM edges e JOIN nodes n ON n.id = e.target_id
                 WHERE e.kind = 'CALLS'
                 GROUP BY n.id ORDER BY callers DESC LIMIT 10"

# All callers of a specific method
ast-graph query "SELECT n.name, n.file_path
                 FROM edges e JOIN nodes n ON n.id = e.source_id
                 WHERE e.target_id = (SELECT id FROM nodes WHERE name = 'MyClass.myMethod')
                 AND e.kind = 'CALLS'"
```

## Cypher Queries (FalkorDB backend)

```bash
# Direct callers of a function, with call-site lines
ast-graph --backend falkor query "
  MATCH (caller:Symbol)-[r:CALLS]->(target:Symbol {name:'MyService.login'})
  RETURN caller.name, caller.file_path, r.line
  ORDER BY caller.file_path, r.line"

# Multi-hop call chain (up to 3 levels) with per-hop line numbers
ast-graph --backend falkor query "
  MATCH path = (a:Symbol {name:'MyService.login'})-[r:CALLS*1..3]->(b:Symbol)
  RETURN [n IN nodes(path) | n.name] AS names,
         [rel IN relationships(path) | rel.line] AS lines,
         length(path) AS depth
  ORDER BY depth"

# Classes implementing an interface (transitive)
ast-graph --backend falkor query "
  MATCH (impl:Symbol)-[:IMPLEMENTS*1..5]->(iface:Symbol {name:'IContainer'})
  RETURN DISTINCT impl.name, impl.file_path"

# Shortest path between two symbols
# NB: FalkorDB requires shortestPath() inside WITH or RETURN, not MATCH
ast-graph --backend falkor query "
  MATCH (a:Symbol {name:'A'}), (b:Symbol {name:'B'})
  WITH shortestPath((a)-[*..10]-(b)) AS p
  RETURN [n IN nodes(p) | n.name], length(p)"
```

## Architecture

```
ast-graph/
  crates/
    ast-graph-core/       Core types: SymbolNode, Edge, CodeGraph, NodeId
    ast-graph-parse/      tree-sitter parsing + per-language extractors
    ast-graph-resolve/    Cross-file import/call/type resolution
    ast-graph-storage/    Pluggable backends: sqlite (recursive CTEs) + falkor (Cypher)
                          behind a single GraphStorage trait
    ast-graph-cli/        CLI binary (clap)
```

### Processing Pipeline

```
Source Files
    │
    ▼
tree-sitter Parse (parallel, rayon)
    │
    ▼
AST Compress — keep only structural nodes (fn, class, import, call)
    │
    ▼
Build Graph — nodes = symbols, edges = raw relationships
    │
    ▼
Cross-file Resolve — match call targets across files
  1. Exact match on full qualified name (e.g. "ClassName.method")
  2. Strip :: namespace prefix (Rust)
  3. Name-only fallback
    │
    ▼
Persist via GraphStorage trait
  ├─ SQLite  → nodes / edges / file_hashes tables
  └─ FalkorDB → :Symbol / :FileHash nodes, typed relationships with r.line
```

### Resolution Quality

Methods are stored as `ClassName.methodName` for all languages. Call targets are qualified at extraction time:

- `this.save()` inside `MyComponent` → stored as `MyComponent.save` → exact match in resolver
- `self.process()` inside `MyService` → stored as `MyService.process` → exact match
- `this.dialog.open()` (chained) → falls back to name-only

This eliminates the false-positive explosion where `this.save()` would previously match every `save()` method in the codebase.

### SQLite Schema

```sql
CREATE TABLE nodes (
    id          TEXT PRIMARY KEY,   -- stable hash of (file, name, kind, line)
    name        TEXT NOT NULL,      -- qualified: "ClassName.methodName"
    kind        TEXT NOT NULL,      -- Function, Class, Method, Struct, Trait, etc.
    file_path   TEXT NOT NULL,
    line_start  INTEGER NOT NULL,
    line_end    INTEGER NOT NULL,
    signature   TEXT,
    doc_comment TEXT,
    visibility  TEXT NOT NULL,
    language    TEXT NOT NULL,
    parent_id   TEXT                -- NodeId of containing class/module
);

CREATE TABLE edges (
    source_id   TEXT NOT NULL REFERENCES nodes(id),
    target_id   TEXT NOT NULL REFERENCES nodes(id),
    kind        TEXT NOT NULL       -- CALLS, IMPORTS, EXTENDS, IMPLEMENTS, CONTAINS, REFERENCES
);

CREATE TABLE file_hashes (
    file_path   TEXT PRIMARY KEY,
    hash        BLOB NOT NULL       -- SHA-256, used for incremental re-scan
);
```

Graph traversal uses SQLite recursive CTEs:

```sql
-- Call chain: what does openTeamOnSet() call, 3 levels deep?
WITH RECURSIVE call_tree(id, name, kind, depth) AS (
    SELECT n.id, n.name, n.kind, 0 FROM nodes n WHERE n.name = 'TeamOnSetService.openTeamOnSet'
    UNION ALL
    SELECT n.id, n.name, n.kind, ct.depth + 1
    FROM call_tree ct
    JOIN edges e ON e.source_id = ct.id AND e.kind = 'CALLS'
    JOIN nodes n ON n.id = e.target_id
    WHERE ct.depth < 3
)
SELECT DISTINCT name, kind, depth FROM call_tree WHERE depth > 0 ORDER BY depth, name;
```

### FalkorDB Schema

FalkorDB stores the same data as a property graph. Every symbol is a `:Symbol` node; file-content hashes live on `:FileHash` nodes (used only for incremental-scan bookkeeping — ignore them when analyzing code).

| `:Symbol` property | Type | Notes |
|---|---|---|
| `id` | string | 16-hex-char stable hash |
| `name` | string | qualified: `ClassName.methodName` |
| `kind` | string | `File`, `Class`, `Method`, `Function`, `Interface`, `Package`, … (see [SymbolKind](crates/ast-graph-core/src/symbol.rs)) |
| `file_path` | string | absolute path |
| `line_start` / `line_end` | int | definition range |
| `signature` / `doc_comment` | string? | may be null |
| `visibility` | string | `Public`, `Private`, `Protected`, `Internal` |
| `language` | string | `rust`, `python`, `javascript`, `typescript`, `csharp`, `java` |

Relationship types: `CALLS`, `CONTAINS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `OVERRIDES`. Every relationship carries a `line` property — **the source line where the relationship originates** (call site for `CALLS`, not the callee's definition line). `line` is `0` when no meaningful line exists (mostly structural `CONTAINS`).

## Languages Supported

| Language | Extensions | What's Extracted |
|---|---|---|
| **Rust** | `.rs` | fn, struct, enum, trait, impl, use, mod, const, static |
| **Python** | `.py` | def, class, import, from...import |
| **JavaScript/TypeScript** | `.js/.ts/.tsx` | function, class, arrow fn, import, interface, enum, type alias |
| **C# (.NET)** | `.cs` | class, method, constructor, interface, using, namespace, record, enum |
| **Java** | `.java` | class, interface, enum, record, method, constructor, field, import, package, extends, implements |

## Edge Types

| Edge | Created by | Notes |
|---|---|---|
| `CALLS` | Function/method call expressions | Qualified at extraction: `this.x()` → `ClassName.x` |
| `IMPORTS` | import / using statements | Path and name-based |
| `EXTENDS` | Class inheritance | Works across files |
| `IMPLEMENTS` | Interface/trait implementation | Works across files |
| `CONTAINS` | Parent-child hierarchy | Rust only (via explicit edges); all others use `parent_id` |
| `REFERENCES` | `new Type()` constructor calls | C# and Java |
| `OVERRIDES` | Method overrides a parent-class method | Emitted where resolver can determine override relationships |

## Building

Requires Rust 1.70+. No other prerequisites to build or run the SQLite backend.

```bash
cargo build --release
```

The binary is self-contained for the SQLite backend — SQLite is bundled via `rusqlite` with the `bundled` feature. The FalkorDB backend is also compiled in (via the `falkordb` crate with `tokio`) and only needs a reachable FalkorDB server at runtime; nothing extra to install on the client side.

## License

**Dual-Use License.** ast-graph is distributed under a dual-use license — see [LICENSE](LICENSE) for the full terms.

- **Non-commercial use (free).** Personal, academic, and research use is free. You may use, copy, modify, and distribute the software for any non-commercial purpose, provided the copyright notice is preserved.
- **Commercial use (paid).** Any for-profit use — including use inside a for-profit organization, integration into a paid product or service, internal tooling that supports revenue-generating operations, and SaaS / consulting / agency deployments — requires a paid license.

Commercial tiers:

| Tier | Price | Scope |
|---|---|---|
| Builder | $79 | 1 developer |
| Studio | $349 | up to 5 developers |
| Platform | $1,999 | org-wide internal deployment |

For commercial licensing, open an issue at [github.com/emtyty/ast-graph/issues](https://github.com/emtyty/ast-graph/issues).

The software is provided "as is", without warranty of any kind — see the disclaimer in [LICENSE](LICENSE).
