//! Route extraction — finds REST/HTTP route declarations across multiple
//! frameworks and emits `Route` symbol nodes plus `HandlesRoute` raw edges
//! from the enclosing handler symbol to each route.
//!
//! Approach: regex-scan the source for known framework patterns (Express,
//! NestJS, FastAPI/Flask, ASP.NET, Spring, Axum/Actix, chi/echo, Laravel,
//! Slim, Symfony). For each match we identify the enclosing function/method
//! by line containment over already-extracted symbols. Pragmatic — covers
//! the common cases without per-language tree-sitter walks.

use ast_graph_core::*;
use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

/// Compiled regex bundle. Built once on first use; subsequent calls are O(1).
struct RoutePatterns {
    /// Express / Fastify / Koa / Hono / Bun: `app.get('/path', handler)` or
    /// `router.post("/path", ...)`. Captures the verb and path.
    express: Regex,
    /// NestJS / TypeScript decorators: `@Get('/path')`, `@Post(...)`, etc.
    /// Also matches `@HttpGet`, `@HttpPost` (some niche TS frameworks).
    nest_decorator: Regex,
    /// FastAPI / Flask: `@app.get("/path")`, `@app.route('/path')`,
    /// `@router.post("/path")`.
    python_decorator: Regex,
    /// Spring Boot: `@GetMapping("/path")`, `@PostMapping(...)`,
    /// `@RequestMapping("/path", method = RequestMethod.GET)`.
    spring: Regex,
    /// Spring `@RequestMapping(...)` with explicit method=. Matches the path
    /// once; method default detection elsewhere.
    spring_request: Regex,
    /// ASP.NET attributes: `[HttpGet("/path")]`, `[HttpPost(...)]`, `[Route("/path")]`.
    aspnet: Regex,
    /// Axum: `Router::new().route("/path", get(handler))` —
    /// captures the path and the verb.
    axum_route: Regex,
    /// Actix attribute macro: `#[get("/path")]`, `#[post(...)]`.
    actix_attr: Regex,
    /// Go chi/echo/gin: `r.Get("/path", handler)`, `e.POST(...)`, `g.GET(...)`.
    go_method: Regex,
    /// Go net/http: `r.HandleFunc("/path", handler)`. Verb unknown → "ANY".
    go_handlefunc: Regex,
    /// Laravel: `Route::get|post|put|delete|patch|any|match('/path', ...)`.
    php_laravel: Regex,
    /// Slim / Lumen: `$app->get|post(...)('/path', ...)`. PHP uses `->` not `.`.
    php_slim: Regex,
    /// Symfony PHP-attribute syntax: `#[Route('/path', methods: ['GET'])]`.
    /// Captures path; verb is parsed separately in the PHP arm.
    php_symfony_attr: Regex,
}

fn patterns() -> &'static RoutePatterns {
    static PATTERNS: OnceLock<RoutePatterns> = OnceLock::new();
    PATTERNS.get_or_init(|| RoutePatterns {
        express: Regex::new(
            r#"(?m)\b(?:app|router|server|api|http)\s*\.\s*(get|post|put|delete|patch|options|head|all|use)\s*\(\s*[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        nest_decorator: Regex::new(
            r#"@(Get|Post|Put|Delete|Patch|Options|Head|All|HttpGet|HttpPost|HttpPut|HttpDelete|HttpPatch)\s*\(\s*[`'"]([^`'"]*)[`'"]"#,
        )
        .unwrap(),
        python_decorator: Regex::new(
            r#"@\s*(?:[A-Za-z_][\w\.]*)\.\s*(get|post|put|delete|patch|options|head|route)\s*\(\s*[rRbBuU]?[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        spring: Regex::new(
            r#"@(GetMapping|PostMapping|PutMapping|DeleteMapping|PatchMapping)\s*\(\s*(?:value\s*=\s*)?[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        spring_request: Regex::new(
            r#"@RequestMapping\s*\(\s*(?:value\s*=\s*)?[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        aspnet: Regex::new(
            r#"\[\s*(HttpGet|HttpPost|HttpPut|HttpDelete|HttpPatch|HttpOptions|HttpHead|Route)\s*\(\s*[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        axum_route: Regex::new(
            r#"\.\s*route\s*\(\s*[`'"]([^`'"]+)[`'"]\s*,\s*(get|post|put|delete|patch|options|head)\b"#,
        )
        .unwrap(),
        actix_attr: Regex::new(
            r##"#\s*\[\s*(get|post|put|delete|patch|options|head)\s*\(\s*[`'"]([^`'"]+)[`'"]"##,
        )
        .unwrap(),
        go_method: Regex::new(
            r#"\b\w+\s*\.\s*(Get|Post|Put|Delete|Patch|Options|Head|GET|POST|PUT|DELETE|PATCH|OPTIONS|HEAD)\s*\(\s*[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        go_handlefunc: Regex::new(
            r#"\b\w+\s*\.\s*HandleFunc\s*\(\s*[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        php_laravel: Regex::new(
            r#"\bRoute::\s*(get|post|put|delete|patch|options|head|any|match)\s*\(\s*[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        php_slim: Regex::new(
            r#"\$\w+\s*->\s*(get|post|put|delete|patch|options|head|any|map)\s*\(\s*[`'"]([^`'"]+)[`'"]"#,
        )
        .unwrap(),
        php_symfony_attr: Regex::new(
            r#"#\s*\[\s*Route\s*\(\s*[`'"]([^`'"]+)[`'"](?:[^\]]*methods\s*:\s*\[\s*[`'"]([A-Za-z]+)[`'"])?"#,
        )
        .unwrap(),
    })
}

/// Walk the source for route patterns and emit Route nodes + HandlesRoute
/// raw edges from the enclosing handler. `existing_symbols` is consulted to
/// find the handler that contains each match line.
pub fn extract_routes(
    source: &[u8],
    file_path: &Path,
    language: Language,
    existing_symbols: &[SymbolNode],
    extra_symbols: &mut Vec<SymbolNode>,
    extra_raw_edges: &mut Vec<RawEdge>,
) {
    let Ok(text) = std::str::from_utf8(source) else {
        return;
    };
    let p = patterns();
    let mut emitted: Vec<(String, u32)> = Vec::new(); // (name, line) dedup

    let emit = |verb: &str, path: &str, byte_offset: usize| {
        let verb_norm = verb.to_uppercase();
        let path_norm = path.trim();
        if path_norm.is_empty() {
            return;
        }
        let name = format!("{} {}", verb_norm, path_norm);
        let line = byte_to_line(text, byte_offset);
        if emitted.iter().any(|(n, l)| n == &name && *l == line) {
            return;
        }
        emitted.push((name.clone(), line));
        let id = NodeId::new(
            &file_path.to_string_lossy(),
            &name,
            SymbolKind::Route,
            line,
        );
        extra_symbols.push(SymbolNode {
            id,
            name: name.clone(),
            kind: SymbolKind::Route,
            file_path: file_path.to_path_buf(),
            line_range: (line, line),
            signature: Some(format!("route {}", name)),
            doc_comment: None,
            visibility: Visibility::Public,
            language,
            parent: None,
        });
        // Find the enclosing handler symbol (Function/Method whose line range
        // contains this line). Pick the innermost (smallest range) match.
        let handler = existing_symbols
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
                ) && s.line_range.0 <= line
                    && line <= s.line_range.1
            })
            .min_by_key(|s| s.line_range.1.saturating_sub(s.line_range.0));
        if let Some(handler) = handler {
            extra_raw_edges.push(RawEdge {
                source: handler.id,
                kind: EdgeKind::HandlesRoute,
                target_name: id.to_string(),
                target_module: None,
                source_line: line,
            });
        }
    };

    let run_pattern = |re: &Regex,
                       text: &str,
                       verb_idx: usize,
                       path_idx: usize,
                       emit: &mut dyn FnMut(&str, &str, usize)| {
        for m in re.captures_iter(text) {
            if let (Some(verb), Some(path)) = (m.get(verb_idx), m.get(path_idx)) {
                emit(verb.as_str(), path.as_str(), verb.start());
            }
        }
    };

    let mut emit_fn = emit;

    match language {
        Language::JavaScript | Language::TypeScript => {
            run_pattern(&p.express, text, 1, 2, &mut emit_fn);
            run_pattern(&p.nest_decorator, text, 1, 2, &mut emit_fn);
        }
        Language::Python => {
            run_pattern(&p.python_decorator, text, 1, 2, &mut emit_fn);
        }
        Language::Java => {
            run_pattern(&p.spring, text, 1, 2, &mut emit_fn);
            // RequestMapping has only the path captured; emit as ANY.
            for m in p.spring_request.captures_iter(text) {
                if let Some(path) = m.get(1) {
                    emit_fn("ANY", path.as_str(), path.start());
                }
            }
        }
        Language::CSharp => {
            run_pattern(&p.aspnet, text, 1, 2, &mut emit_fn);
        }
        Language::Rust => {
            // Axum: path is capture 1, verb is capture 2. Swap idx.
            for m in p.axum_route.captures_iter(text) {
                if let (Some(path), Some(verb)) = (m.get(1), m.get(2)) {
                    emit_fn(verb.as_str(), path.as_str(), path.start());
                }
            }
            run_pattern(&p.actix_attr, text, 1, 2, &mut emit_fn);
        }
        Language::Swift => {
            // Vapor is the main Swift web framework, but adoption is niche.
            // Skip route extraction for Swift until a real codebase shows up.
        }
        Language::Php => {
            run_pattern(&p.php_laravel, text, 1, 2, &mut emit_fn);
            run_pattern(&p.php_slim, text, 1, 2, &mut emit_fn);
            // Symfony attributes: path is capture 1, optional verb is capture 2.
            for m in p.php_symfony_attr.captures_iter(text) {
                if let Some(path) = m.get(1) {
                    let verb = m
                        .get(2)
                        .map(|v| v.as_str().to_string())
                        .unwrap_or_else(|| "ANY".to_string());
                    emit_fn(&verb, path.as_str(), path.start());
                }
            }
        }
        Language::Go => {
            run_pattern(&p.go_method, text, 1, 2, &mut emit_fn);
            for m in p.go_handlefunc.captures_iter(text) {
                if let Some(path) = m.get(1) {
                    emit_fn("ANY", path.as_str(), path.start());
                }
            }
        }
    }
}

fn byte_to_line(text: &str, byte: usize) -> u32 {
    if byte >= text.len() {
        return text.bytes().filter(|&b| b == b'\n').count() as u32;
    }
    text[..byte].bytes().filter(|&b| b == b'\n').count() as u32
}
