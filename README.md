# ast-graph

A fast codebase intelligence tool. Uses **tree-sitter** for multi-language AST parsing and a pluggable storage backend — **SQLite** (zero-setup, single file) or **FalkorDB** (Redis-module graph database, OpenCypher). Parse any codebase → extract its structural skeleton (functions, classes, imports, calls) → query it instantly from the CLI.

![ast-graph demo](docs/ast-graph-demo.gif)

```bash
ast-graph scan ./my-project
ast-graph symbol "MyService"
ast-graph hotspots
```

## Features

- **Multi-language** — Rust, Python, JavaScript/TypeScript, C# (.NET), Java, Go
- **AST compression** — strips full syntax trees down to structural nodes only (~90% reduction)
- **Class-context-aware resolution** — `this.method()` / `self.method()` calls resolve to the correct class, not every method with that name across the codebase
- **Cross-file resolution** — resolves function calls, imports, type references, inheritance across files
- **Confidence-tiered edges** — every edge carries a `confidence` score (1.0 exact / 0.95 same-file / 0.9 import-scoped / 0.5 global fallback) so queries can filter noisy matches
- **Python C3 MRO** — `super().method()` resolves to the correct ancestor via C3 linearization, not a name-fan-out across the codebase
- **Overload-safe IDs** — `NodeId` includes a signature digest for callable kinds, so two overloads of the same method don't collide
- **Symbol lookup** — find any symbol by partial name, instantly see callers, callees, members
- **Pluggable backends** — SQLite for single-machine use, FalkorDB for team/server use with Cypher
- **Git-aware analysis** — `blast-radius`, `changed-symbols`, and `dead-code` bring PR review and refactor planning onto the graph
- **HTTP route extraction** — `Route` nodes for Express, NestJS, FastAPI/Flask, Spring Boot, ASP.NET, Axum, Actix, chi/echo/gin, net/http
- **Process tracing** — entry-point detection (`main`, route handlers, `test_*` functions) plus a depth-bounded BFS along CALLS edges, surfaced as `Process` nodes with `STEP_IN_PROCESS` edges
- **Full-text search** — bundled SQLite FTS5 over name + signature + doc comment, BM25-ranked, no extra deps
- **MCP server** — `ast-graph mcp` speaks JSON-RPC over stdio so Claude Code, Cursor, Codex, Windsurf, and OpenCode can call ast-graph natively
- **SQL / Cypher escape hatch** — run arbitrary SQL (SQLite) or Cypher (FalkorDB) against the graph
- **AI context export** — compact skeleton format for feeding into LLMs
- **Doc comments captured by default** — function/class/method docstrings, JSDoc, `///` comments, JavaDoc, etc. are stored on every symbol; pass `--no-doc-comments` to skip them for a leaner graph
- **Self-contained default** — SQLite backend needs no Docker or external services; single binary + `.db` file
- **Incremental scan** — only re-parses changed files on re-scan

## ast-graph + Claude vs. Claude alone

Benchmark on a real C# codebase (810 files, ~53k LOC, 2026-04-18). Both approaches were asked to produce the same high-level map of the repo.

| | Claude default tools | ast-graph + FalkorDB |
|---|---|---|
| Time | 10 min 28 sec | **5 min 38 sec** |
| Tokens | 84.4k | 86.7k |
| Tool calls | 88 | 82 |

Both approaches agreed on the structural facts: 810 files, ~1,350 classes, 118 interfaces, ~1,570 methods, 19 REST + 1 gRPC controller, 16 CQRS features, ~48 event handlers, ~48 Kafka consumers, 8 background jobs, 39 enums.

### What each approach is good at

**Claude default tools (grep / read / edit)**
- Works on any repo instantly, no setup
- Understands intent — comments, naming, style
- Follows clues outside code (e.g. `CLAUDE.md`)
- Actually writes and edits code

**ast-graph + FalkorDB**
- Fast — queries answer in milliseconds
- Finds hotspots across the whole codebase
- Exact counts, no grep false-positives
- Traces long call chains in one query

### Best approach per action

| Action | Best approach | Why |
|---|---|---|
| Explore a codebase | ast-graph | One query, whole-map overview |
| Find dead code / cycles | ast-graph | Unreferenced nodes show up instantly |
| Refactor / rename | Claude + ast-graph | Graph lists every caller; Claude edits safely |
| Write new features | Claude + ast-graph | Graph shows existing patterns; Claude writes the code |
| Fix bugs | Claude + ast-graph | Graph traces the call chain; Claude patches the file |
| Unit / integration tests | Claude + ast-graph | Graph maps deps; Claude writes the tests |
| Code review | Claude + ast-graph | Graph spots structural smells; Claude reads the prose |
| Architecture docs | Claude + ast-graph | Graph draws the shape; Claude tells the story |

**The combined workflow wins.** Claude reads the graph (via MCP or a system prompt like [graph/CLAUDE.md](../CLAUDE.md)) for exact structure, then writes code. Almost always beats either alone.

### Example hotspots surfaced in that benchmark

```
139 OUTGOING   UpdatePartialSessionCommandHandler.Handle   top orchestrator; #1 split candidate
278 INCOMING   SocketHelper.Send                           most-called method in the repo
483 LINES      UpsertSessionCommandHandler                 largest handler; publishes 3+ events mid-flow
```

The first two are trivially surfaced by a single Cypher query over `size(()-->(n))` / `size((n)-->())`; the third needs `line_end - line_start` over the graph.

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
ast-graph scan <path> [--clean] [--no-doc-comments]
                                      Scan a directory and build the code graph
ast-graph symbol <name>               Look up a symbol — callers, callees, members
ast-graph hotspots [--limit 20]       Most connected symbols (architectural hotspots)
ast-graph call-chain <name>           Trace call chain from a function (recursive)
ast-graph blast-radius <name>         Reverse traversal: "if I change X, what else breaks?"
ast-graph changed-symbols [--base <ref>]
                                      Map a git diff to the symbols it actually touched
ast-graph dead-code                   Functions / methods with zero incoming CALLS edges
ast-graph search "<query>"            Full-text keyword search (BM25 over name+sig+docs)
ast-graph routes                      List extracted HTTP routes
ast-graph processes                   List traced execution flows (entry point + step count)
ast-graph mcp                         Run as an MCP server over stdio
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

## Git-aware analysis

Three commands use the graph to answer questions that grep can't, tied to what you're changing right now.

### `blast-radius` — "if I change X, what else breaks?"

Reverse traversal of the CALLS graph from any symbol, N hops upstream. Lists every caller and ranks files by how many callers they contain.

```bash
ast-graph blast-radius TokenService.validate --depth 3

# Add --with-recency to annotate each caller's file with git churn
# (commit count in the last 30 days, plus "last touched" age)
ast-graph blast-radius TokenService.validate --depth 2 --with-recency
ast-graph blast-radius TokenService.validate --with-recency --recency-days 90
```

The "recency" signal is the interesting part: a symbol with 200 callers that hasn't changed in two years is architecturally stable. A symbol with 200 callers that was touched 11 times in the last month is **on fire** — same blast radius, much higher review priority.

### `changed-symbols` — map a git diff to symbols

Reads `git diff --unified=0`, extracts every hunk, and looks up which symbols' line ranges overlap.

```bash
# Diff the working tree against HEAD (uncommitted changes)
ast-graph changed-symbols

# Diff against a base ref — typical use case in PR review
ast-graph changed-symbols --base origin/main

# Also list direct callers of each changed symbol
ast-graph changed-symbols --base origin/main --callers
```

Example output:

```
Changed symbols (origin/main..HEAD) — 12 symbols across 4 file(s):

src/services/user.service.ts
  L   42-78    Method       UserService.login
  L   80-95    Method       UserService.refreshToken
src/services/token.service.ts
  L   22-40    Method       TokenService.issue
  L   42-55    Method       TokenService.validate
...
```

A reviewer instantly sees that a 300-line diff actually reshapes exactly 12 symbols, concentrated in the auth flow — faster and more accurate than scrolling the diff.

### `dead-code` — functions with no inbound calls

Pure graph query — any Function / Method / Constructor with zero incoming `CALLS` edges is flagged as likely dead.

```bash
ast-graph dead-code                        # default: 200 results, excludes vendored files
ast-graph dead-code --limit 50
ast-graph dead-code --kinds Function,Method,Constructor
ast-graph dead-code --include-all          # disable vendored-file exclusions
```

**Interpret with care.** "Likely dead" is a *graph-level* claim and has three known failure modes, which the command prints as caveats:

- **Entry points** (`main`, HTTP handlers, `#[test]` functions, event listeners) have no callers by design.
- **Dynamic dispatch** — virtual calls, reflection, JS callbacks, framework hooks — is invisible to the graph. Anything invoked through `this.fn.call()`, `QueryList.forEach`, or a trait object's method table won't resolve.
- **Library APIs** may be called by external consumers the graph never saw.

Review the list as a candidate set, not a deletion list.

## Confidence-tiered edges

Every resolved edge carries a `confidence: f32` field set at resolution time:

| Tier | Value | What it means |
|---|---|---|
| Exact | `1.0` | Structural (parent→child CONTAINS), by-path import resolution, route NodeId-encoded targets, Python C3 MRO super() match |
| Same-file | `0.95` | Caller and target live in the same file — no cross-file ambiguity |
| Import-scoped | `0.9` | Single global hit (the name resolves to exactly one symbol) |
| Global fallback | `0.5` | Multiple-target name match — last-segment guess, may fan out |

This lets queries trim noise:

```bash
# Only high-confidence edges
ast-graph query "SELECT n1.name, n2.name FROM edges e
                 JOIN nodes n1 ON n1.id = e.source_id
                 JOIN nodes n2 ON n2.id = e.target_id
                 WHERE e.kind = 'CALLS' AND e.confidence >= 0.9"

# How fuzzy is the graph?
ast-graph query "SELECT confidence, COUNT(*) FROM edges
                 GROUP BY confidence ORDER BY confidence DESC"
```

The `0.5` tier is the honest "we matched on name only" bucket — it's where dynamic dispatch, dotted-call chains, and overload fan-out land.

## HTTP routes and process tracing

`scan` automatically extracts HTTP routes from the source and traces execution flows from entry points. Two new node kinds join the graph: `Route` (e.g. `GET /users`) and `Process` (an entry point + its call-chain BFS).

### `routes` — list extracted HTTP endpoints

Frameworks recognized out of the box:

| Language | Frameworks |
|---|---|
| TypeScript / JavaScript | Express, Fastify, Koa, Hono, Bun, NestJS decorators |
| Python | FastAPI / Flask `@app.<verb>(...)`, `@app.route(...)` |
| Java | Spring Boot `@GetMapping`, `@PostMapping`, `@RequestMapping`, … |
| C# | ASP.NET `[HttpGet("/x")]`, `[HttpPost(...)]`, `[Route("/x")]` |
| Rust | Axum `Router::new().route(...)`, Actix `#[get("/x")]` |
| Go | chi/echo/gin `r.Get(...)`, `r.HandleFunc(...)` |

```bash
ast-graph routes
# Routes (12):
#   GET /users          1 handler(s)  src/api/users.ts:14
#   POST /login         1 handler(s)  src/api/auth.ts:42
#   …
```

Each `Route` node is connected to its handler (the enclosing Function/Method) by a `HANDLES_ROUTE` edge, so you can ask "what calls a route handler?" or "which routes does this service expose?" with a single Cypher / SQL query.

### `processes` — execution flows from entry points

Entry-point detection covers `main` / `Main`, route handlers (every `Route` is an entry), and test functions (`test_*`, `Test*`, `*Test`). From each entry the tracer does a depth-bounded BFS along `CALLS` edges (default depth 6, max 50 steps) and emits:

- a `Process` node per entry point (`name = "Process: <entry>"`)
- an `ENTRY_POINT_OF` edge from the entry symbol to the process
- a `STEP_IN_PROCESS` edge from each step to the process, with the step index encoded in `source_line`

```bash
ast-graph processes --limit 10
# Processes (10):
#   Process: GET /users         12 step(s)  src/api/users.ts:14
#   Process: POST /login         9 step(s)  src/api/auth.ts:42
#   Process: main               34 step(s)  src/main.ts:5
#   …
```

This gives you a one-query answer to "what does this endpoint actually do?" — the steps along the process are the real call chain, not a dump of the file.

## Full-text search

A SQLite FTS5 virtual table is kept in sync with `nodes` (name + signature + doc_comment). BM25-ranked, no extra deps — bundled with rusqlite.

```bash
ast-graph search "auth login token"
# Top 7 matches for: auth login token
#   1. [     Method] AuthService.validate (src/auth/service.ts:4) — score -2.234
#   2. [      Route] POST /login          (src/api/auth.ts:21)   — score -2.080
#   3. [     Method] AuthService.login    (src/auth/service.ts:7) — score -1.986
#   …
```

Lower scores rank better (FTS5 BM25 convention). Suffix wildcards are appended automatically, so `auth` also matches `authn`, `authorize`, etc.

## MCP server (Claude Code, Cursor, Codex, Windsurf, OpenCode)

`ast-graph mcp` exposes the graph as a stock MCP server over stdio (JSON-RPC 2.0). Ten tools are registered:

| Tool | Purpose |
|---|---|
| `symbol` | Look up a symbol with optional callers/callees/members |
| `call_chain` | Forward CALLS traversal from a symbol |
| `blast_radius` | Reverse CALLS traversal (who depends on this) |
| `hotspots` | Most connected symbols |
| `dead_code` | Functions with zero incoming CALLS |
| `search` | BM25 keyword search |
| `routes` | List extracted HTTP routes |
| `processes` | List traced execution flows |
| `stats` | Graph summary |
| `query` | Backend-native SQL / Cypher escape hatch |

### Setup for Claude Code

```bash
claude mcp add ast-graph -- ast-graph --db /path/to/.ast-graph/graph.db mcp
```

### Setup for Cursor

`~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "ast-graph": {
      "command": "ast-graph",
      "args": ["--db", "/path/to/.ast-graph/graph.db", "mcp"]
    }
  }
}
```

The server speaks newline-delimited JSON-RPC by default and also accepts LSP-style `Content-Length`-framed messages.

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
- `super().method()` in Python → walks the C3 linearization of the enclosing class and picks the first matching ancestor

This eliminates the false-positive explosion where `this.save()` would previously match every `save()` method in the codebase. Every edge also carries a `confidence` score that records *how* the match was made (see "Confidence-tiered edges" above), so consumers can opt out of fuzzy fallbacks.

**Overload safety.** `NodeId` is `hash(file, name, kind, line)` — for callable kinds (`Method`, `Function`, `Constructor`) the signature is also folded in, so two overloads of `Container.add(int)` and `Container.add(string)` produce distinct IDs even if they share a line.

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
    kind        TEXT NOT NULL,      -- CALLS, IMPORTS, EXTENDS, IMPLEMENTS, CONTAINS,
                                    -- REFERENCES, OVERRIDES, HANDLES_ROUTE,
                                    -- STEP_IN_PROCESS, ENTRY_POINT_OF
    source_line INTEGER NOT NULL,   -- call site line (or step index for STEP_IN_PROCESS)
    confidence  REAL NOT NULL       -- 1.0 / 0.95 / 0.9 / 0.5 — see "Confidence-tiered edges"
);

CREATE TABLE file_hashes (
    file_path   TEXT PRIMARY KEY,
    hash        BLOB NOT NULL       -- SHA-256, used for incremental re-scan
);

-- FTS5 virtual table — kept in sync with `nodes` via triggers.
CREATE VIRTUAL TABLE symbol_fts USING fts5(
    id UNINDEXED, name, signature, doc_comment,
    tokenize='unicode61'
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
| `kind` | string | `File`, `Class`, `Method`, `Function`, `Interface`, `Package`, `Route`, `Process`, … (see [SymbolKind](crates/ast-graph-core/src/symbol.rs)) |
| `file_path` | string | absolute path |
| `line_start` / `line_end` | int | definition range |
| `signature` / `doc_comment` | string? | `doc_comment` populated by default from preceding `///`, `/** */`, JSDoc, JavaDoc, or Python docstrings; null when source has no doc or `--no-doc-comments` was used |
| `visibility` | string | `Public`, `Private`, `Protected`, `Internal` |
| `language` | string | `rust`, `python`, `javascript`, `typescript`, `csharp`, `java`, `go` |

Relationship types: `CALLS`, `CONTAINS`, `IMPORTS`, `EXTENDS`, `IMPLEMENTS`, `REFERENCES`, `OVERRIDES`, `HANDLES_ROUTE`, `STEP_IN_PROCESS`, `ENTRY_POINT_OF`. Every relationship carries a `line` property — **the source line where the relationship originates** (call site for `CALLS`, step index for `STEP_IN_PROCESS`, not the callee's definition line). `line` is `0` when no meaningful line exists (mostly structural `CONTAINS`). Every relationship also carries a `confidence` property in `[0.5, 1.0]`.

## Languages Supported

| Language | Extensions | What's Extracted |
|---|---|---|
| **Rust** | `.rs` | fn, struct, enum, trait, impl, use, mod, const, static |
| **Python** | `.py` | def, class, import, from...import |
| **JavaScript/TypeScript** | `.js/.ts/.tsx` | function, class, arrow fn, import, interface, enum, type alias |
| **C# (.NET)** | `.cs` | class, method, constructor, interface, using, namespace, record, enum |
| **Java** | `.java` | class, interface, enum, record, method, constructor, field, import, package, extends, implements |
| **Go** | `.go` | package, func, method (pointer + value receivers), struct, interface, type alias, import, const, field |

## Edge Types

| Edge | Created by | Notes |
|---|---|---|
| `CALLS` | Function/method call expressions | Qualified at extraction: `this.x()` → `ClassName.x`. `super().X` in Python uses C3 MRO. |
| `IMPORTS` | import / using statements | Path and name-based |
| `EXTENDS` | Class inheritance | Works across files |
| `IMPLEMENTS` | Interface/trait implementation | Works across files |
| `CONTAINS` | Parent-child hierarchy | Rust only (via explicit edges); all others use `parent_id` |
| `REFERENCES` | `new Type()` constructor calls | C# and Java |
| `OVERRIDES` | Method overrides a parent-class method | Emitted where resolver can determine override relationships |
| `HANDLES_ROUTE` | Handler symbol → `Route` node | Emitted by the route extractor for Express, NestJS, FastAPI, Spring, ASP.NET, Axum, Actix, chi/echo, etc. |
| `STEP_IN_PROCESS` | Symbol → `Process` node | Each step in a traced execution flow; `source_line` carries the 1-based step index |
| `ENTRY_POINT_OF` | Entry-point symbol → `Process` node | Marks the root of a process |

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
