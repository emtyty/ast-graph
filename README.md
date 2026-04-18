# ast-graph

A fast, self-contained codebase intelligence tool. Uses **tree-sitter** for multi-language AST parsing and **SQLite** for storage. Parse any codebase → extract its structural skeleton (functions, classes, imports, calls) → query it instantly from the CLI.

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
- **SQL escape hatch** — run arbitrary SQL against the graph database
- **AI context export** — compact skeleton format for feeding into LLMs
- **Zero external dependencies** — no Docker, no external databases, single binary + SQLite file
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
ast-graph query "<sql>"               Run a SQL query against the graph DB
ast-graph stats                       Graph summary: nodes, edges, languages
ast-graph export --format <fmt>       Export graph (json, dot, ai-context)
```

All commands accept `--db <path>` to point at a specific database file.

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

## SQL Queries

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

## Architecture

```
ast-graph/
  crates/
    ast-graph-core/       Core types: SymbolNode, Edge, CodeGraph, NodeId
    ast-graph-parse/      tree-sitter parsing + per-language extractors
    ast-graph-resolve/    Cross-file import/call/type resolution
    ast-graph-storage/    SQLite persistence + graph queries (recursive CTEs)
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
Store in SQLite — nodes, edges, file_hashes tables
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
| `REFERENCES` | `new Type()` constructor calls | C# only |

## Building

Requires Rust 1.70+. No other prerequisites.

```bash
cargo build --release
```

The binary is fully self-contained — SQLite is bundled via `rusqlite` with the `bundled` feature.

## License

MIT
