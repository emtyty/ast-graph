//! File-based route detection — Next.js App Router (`app/**/page.tsx`),
//! Pages Router (`pages/**/*.tsx`), Remix (`routes/**`), Nuxt 3 (`pages/**/*.vue`),
//! SvelteKit (`routes/**/+page.svelte`).
//!
//! Runs as a project-level pre-pass *before* resolution. Walks the file index
//! built up by per-file parsing, identifies file-based routes by path
//! convention, and emits `Route` symbol nodes plus `HANDLES_ROUTE` raw edges
//! to the file-level handler symbol when one is detectable.

use ast_graph_core::*;

/// Walk the graph's file index and emit Route nodes for file-based routing
/// conventions found in `project_root` and below.
pub fn detect_filebased_routes(graph: &mut CodeGraph) {
    // Collect everything we want to add up front so we don't borrow `graph`
    // mutably and immutably at the same time.
    let mut new_nodes: Vec<SymbolNode> = Vec::new();
    let mut new_raw_edges: Vec<RawEdge> = Vec::new();

    for (file_path, node_ids) in &graph.file_index {
        let normalized = file_path.to_string_lossy().replace('\\', "/");
        let route_path = match derive_route_path(&normalized) {
            Some(p) => p,
            None => continue,
        };

        // Pick the language from the File node so the Route inherits it.
        let file_node_id = node_ids.iter().find(|id| {
            graph
                .nodes
                .get(id)
                .map_or(false, |n| n.kind == SymbolKind::File)
        });
        let Some(file_node_id) = file_node_id else {
            continue;
        };
        let language = graph
            .nodes
            .get(file_node_id)
            .map(|n| n.language)
            .unwrap_or(Language::TypeScript);

        // Next.js App Router server `route.ts` files export one function per
        // HTTP verb. Emit a Route node per matching exported verb function;
        // skip the synthetic catch-all GET to avoid double-counting.
        let is_app_route_ts = normalized.contains("/app/")
            && (normalized.ends_with("/route.ts")
                || normalized.ends_with("/route.tsx")
                || normalized.ends_with("/route.js"));

        if is_app_route_ts {
            let mut emitted_any = false;
            for verb in &[
                "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS",
            ] {
                let handler = node_ids.iter().find(|id| {
                    graph
                        .nodes
                        .get(id)
                        .map_or(false, |n| {
                            n.name == *verb || n.name.ends_with(&format!(".{verb}"))
                        })
                });
                if let Some(handler_id) = handler {
                    emitted_any = true;
                    let verb_route_name = format!("{} {}", verb, route_path);
                    let verb_id =
                        NodeId::new(&normalized, &verb_route_name, SymbolKind::Route, 0);
                    if !graph.nodes.contains_key(&verb_id) {
                        new_nodes.push(SymbolNode {
                            id: verb_id,
                            name: verb_route_name.clone(),
                            kind: SymbolKind::Route,
                            file_path: file_path.clone(),
                            line_range: (0, 0),
                            signature: Some(format!("route {}", verb_route_name)),
                            doc_comment: None,
                            visibility: Visibility::Public,
                            language,
                            parent: None,
                        });
                    }
                    new_raw_edges.push(RawEdge {
                        source: *handler_id,
                        kind: EdgeKind::HandlesRoute,
                        target_name: verb_id.to_string(),
                        target_module: None,
                        source_line: 0,
                    });
                }
            }
            if emitted_any {
                continue;
            }
            // Fall through: no exported verb functions detected — emit a
            // generic GET route instead of nothing.
        }

        // Page-style routes (Next.js page.tsx, Pages Router, Remix, Nuxt,
        // SvelteKit) — emit a single GET route attributed to the first
        // top-level handler (the default-exported page component).
        let route_name = format!("GET {}", route_path);
        let id = NodeId::new(&normalized, &route_name, SymbolKind::Route, 0);
        if !graph.nodes.contains_key(&id) {
            new_nodes.push(SymbolNode {
                id,
                name: route_name.clone(),
                kind: SymbolKind::Route,
                file_path: file_path.clone(),
                line_range: (0, 0),
                signature: Some(format!("route {}", route_name)),
                doc_comment: None,
                visibility: Visibility::Public,
                language,
                parent: None,
            });
        }
        let handler = node_ids
            .iter()
            .filter_map(|id| graph.nodes.get(id).map(|n| (id, n)))
            .filter(|(_, n)| {
                matches!(
                    n.kind,
                    SymbolKind::Function | SymbolKind::Method | SymbolKind::Constructor
                )
            })
            .min_by_key(|(_, n)| n.line_range.0);
        if let Some((handler_id, _)) = handler {
            new_raw_edges.push(RawEdge {
                source: *handler_id,
                kind: EdgeKind::HandlesRoute,
                target_name: id.to_string(),
                target_module: None,
                source_line: 0,
            });
        }
    }

    for n in new_nodes {
        graph.add_node(n);
    }
    for e in new_raw_edges {
        graph.add_raw_edge(e);
    }
}

/// Convert a file path to a route path, returning `None` if the path is not
/// a recognized file-based route. Path is expected forward-slashed.
fn derive_route_path(normalized: &str) -> Option<String> {
    let lower = normalized.to_ascii_lowercase();

    // SvelteKit: routes/**/+page.svelte → /<rest>
    if lower.contains("/routes/") && lower.ends_with("/+page.svelte") {
        let after = normalized.split("/routes/").last()?;
        let trimmed = after.trim_end_matches("/+page.svelte");
        return Some(svelte_path(trimmed));
    }

    // Next.js App Router: app/**/page.{tsx,jsx,ts,js} → strip /page.<ext>
    if lower.contains("/app/")
        && (lower.ends_with("/page.tsx")
            || lower.ends_with("/page.ts")
            || lower.ends_with("/page.jsx")
            || lower.ends_with("/page.js"))
    {
        let after = normalized.split("/app/").last()?;
        let trimmed = after.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        return Some(nextjs_path(trimmed));
    }

    // Next.js App Router server route: app/**/route.{ts,tsx,js} → strip /route.<ext>
    if lower.contains("/app/")
        && (lower.ends_with("/route.ts")
            || lower.ends_with("/route.tsx")
            || lower.ends_with("/route.js"))
    {
        let after = normalized.split("/app/").last()?;
        let trimmed = after.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
        return Some(nextjs_path(trimmed));
    }

    // Next.js Pages Router: pages/**/*.{tsx,jsx,ts,js} excluding _app, _document, api/.
    if lower.contains("/pages/")
        && (lower.ends_with(".tsx")
            || lower.ends_with(".ts")
            || lower.ends_with(".jsx")
            || lower.ends_with(".js"))
    {
        let after = normalized.split("/pages/").last()?;
        // Skip framework-special files and api routes (those are server,
        // already handled by the Express extractor for export const handler patterns).
        let lower_after = after.to_ascii_lowercase();
        if lower_after.starts_with("_app.")
            || lower_after.starts_with("_document.")
            || lower_after.starts_with("_error.")
            || lower_after.starts_with("api/")
        {
            return None;
        }
        let no_ext = strip_ext(after);
        let cleaned = if no_ext == "index" {
            "".to_string()
        } else {
            no_ext.trim_end_matches("/index").to_string()
        };
        return Some(nextjs_path(&cleaned));
    }

    // Nuxt 3: pages/**/*.vue → /<rest>
    if lower.contains("/pages/") && lower.ends_with(".vue") {
        let after = normalized.split("/pages/").last()?;
        let no_ext = after.trim_end_matches(".vue");
        let cleaned = if no_ext == "index" {
            "".to_string()
        } else {
            no_ext.trim_end_matches("/index").to_string()
        };
        return Some(nextjs_path(&cleaned));
    }

    // Remix: routes/**/*.{tsx,ts,jsx,js} → dot-to-slash for nested.
    if lower.contains("/routes/")
        && (lower.ends_with(".tsx")
            || lower.ends_with(".ts")
            || lower.ends_with(".jsx")
            || lower.ends_with(".js"))
    {
        let after = normalized.split("/routes/").last()?;
        let no_ext = strip_ext(after);
        // `_index` → root; `users.$id` → /users/:id; `users._index` → /users.
        let cleaned = no_ext
            .replace("._index", "")
            .replace("_index", "")
            .replace('.', "/")
            .replace("$", ":");
        let cleaned = cleaned.trim_end_matches('/').to_string();
        return Some(if cleaned.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", cleaned.trim_start_matches('/'))
        });
    }

    None
}

fn nextjs_path(rel: &str) -> String {
    let trimmed = rel.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    // `[id]` → `:id`, `[...slug]` → `:slug*`, `(group)` segments are routing
    // groups that don't appear in the URL.
    let mut out = String::from("/");
    let mut first = true;
    for seg in trimmed.split('/') {
        if seg.starts_with('(') && seg.ends_with(')') {
            continue; // route group
        }
        if !first {
            out.push('/');
        }
        first = false;
        if let Some(inner) = seg.strip_prefix("[...").and_then(|s| s.strip_suffix(']')) {
            out.push(':');
            out.push_str(inner);
            out.push('*');
        } else if let Some(inner) = seg.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            out.push(':');
            out.push_str(inner);
        } else {
            out.push_str(seg);
        }
    }
    out
}

fn svelte_path(rel: &str) -> String {
    let trimmed = rel.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".to_string();
    }
    let mut out = String::from("/");
    let mut first = true;
    for seg in trimmed.split('/') {
        if seg.starts_with('(') && seg.ends_with(')') {
            continue; // svelte (group)
        }
        if !first {
            out.push('/');
        }
        first = false;
        if let Some(inner) = seg.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            out.push(':');
            out.push_str(inner.trim_start_matches("..."));
        } else {
            out.push_str(seg);
        }
    }
    out
}

fn strip_ext(s: &str) -> &str {
    for ext in &[".tsx", ".ts", ".jsx", ".js", ".vue", ".svelte"] {
        if let Some(stripped) = s.strip_suffix(ext) {
            return stripped;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nextjs_app_router() {
        assert_eq!(
            derive_route_path("/repo/app/users/[id]/page.tsx"),
            Some("/users/:id".to_string())
        );
        assert_eq!(
            derive_route_path("/repo/app/page.tsx"),
            Some("/".to_string())
        );
        assert_eq!(
            derive_route_path("/repo/app/(dashboard)/settings/page.tsx"),
            Some("/settings".to_string())
        );
    }

    #[test]
    fn nextjs_pages_router() {
        assert_eq!(
            derive_route_path("/repo/pages/users/[id].tsx"),
            Some("/users/:id".to_string())
        );
        assert_eq!(
            derive_route_path("/repo/pages/index.tsx"),
            Some("/".to_string())
        );
        assert_eq!(derive_route_path("/repo/pages/_app.tsx"), None);
        assert_eq!(derive_route_path("/repo/pages/api/users.ts"), None);
    }

    #[test]
    fn nextjs_app_route_ts() {
        assert_eq!(
            derive_route_path("/repo/app/api/users/route.ts"),
            Some("/api/users".to_string())
        );
    }

    #[test]
    fn remix() {
        assert_eq!(
            derive_route_path("/repo/routes/users.$id.tsx"),
            Some("/users/:id".to_string())
        );
    }

    #[test]
    fn sveltekit() {
        assert_eq!(
            derive_route_path("/repo/routes/users/[id]/+page.svelte"),
            Some("/users/:id".to_string())
        );
    }
}
