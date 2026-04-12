use anyhow::Result;
use std::path::Path;

pub fn run(
    name: &str,
    callers: bool,
    callees: bool,
    members: bool,
    limit: usize,
    db_path: Option<&Path>,
) -> Result<()> {
    let canon = Path::new(".").canonicalize()?;
    let db_file = db_path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| ast_graph_storage::default_db_path(&canon));
    let conn = ast_graph_storage::open_db(&db_file)?;

    // Search for matching symbols
    let matches = ast_graph_storage::find_symbols(&conn, name, limit)?;

    if matches.is_empty() {
        println!("No symbols matching '{}' found.", name);
        return Ok(());
    }

    // If multiple matches and no exact match, show a disambiguation list
    let exact: Vec<_> = matches
        .iter()
        .filter(|m| m["name"].as_str().unwrap_or("").to_lowercase() == name.to_lowercase())
        .collect();

    // Use exact match if unique, otherwise the first (closest) result
    // but also print all matches if > 1 so the user can narrow down
    let show_all_sections = callers || callees || members || matches.len() == 1 || !exact.is_empty();

    if matches.len() > 1 {
        println!("Found {} matches for '{}':\n", matches.len(), name);
        for (i, m) in matches.iter().enumerate() {
            let file = short_path(m["file_path"].as_str().unwrap_or(""));
            println!(
                "  [{}] {} ({}) @ {} L{}",
                i + 1,
                m["name"].as_str().unwrap_or("?"),
                m["kind"].as_str().unwrap_or("?"),
                file,
                m["line_start"].as_i64().unwrap_or(0),
            );
        }
        println!();
    }

    // Show full details for the best match (exact if available, else first)
    let target = if !exact.is_empty() { exact[0] } else { &matches[0] };

    if show_all_sections {
        print_symbol_detail(&conn, target, callers, callees, members)?;
    }

    Ok(())
}

fn print_symbol_detail(
    conn: &rusqlite::Connection,
    node: &serde_json::Value,
    show_callers: bool,
    show_callees: bool,
    show_members: bool,
) -> Result<()> {
    let id = node["id"].as_str().unwrap_or("");
    let name = node["name"].as_str().unwrap_or("?");
    let kind = node["kind"].as_str().unwrap_or("?");
    let file = short_path(node["file_path"].as_str().unwrap_or(""));
    let line_start = node["line_start"].as_i64().unwrap_or(0);
    let line_end = node["line_end"].as_i64().unwrap_or(0);
    let sig = node["signature"].as_str().unwrap_or("");

    // Header
    println!("┌─ {} [{}]", name, kind);
    println!("│  File: {} L{}-{}", file, line_start, line_end);
    if !sig.is_empty() && sig != name {
        // Show first line of signature only
        let sig_line = sig.lines().next().unwrap_or(sig);
        println!("│  Sig:  {}", sig_line);
    }
    println!("│");

    // Members (for classes/interfaces/structs/enums)
    let is_type_node = matches!(kind, "Class" | "Interface" | "Trait" | "Struct" | "Enum" | "Record");
    if is_type_node && (show_members || (!show_callers && !show_callees)) {
        let members = ast_graph_storage::symbol_members(conn, id)?;
        if !members.is_empty() {
            println!("├─ Members ({}):", members.len());
            for m in &members {
                let mname = m["name"].as_str().unwrap_or("?");
                let mkind = m["kind"].as_str().unwrap_or("?");
                let msig = m["signature"].as_str().unwrap_or(mname);
                let sig_line = msig.lines().next().unwrap_or(msig);
                // Strip the "ClassName." prefix for display since we're already showing the class
                let display_name = mname.splitn(2, '.').nth(1).unwrap_or(mname);
                let display_sig = if sig_line.contains('(') || sig_line.contains(':') {
                    sig_line.to_string()
                } else {
                    display_name.to_string()
                };
                println!(
                    "│  [{:11}] {}",
                    mkind,
                    truncate(&display_sig, 80),
                );
            }
            println!("│");
        }
    }

    // Callers
    if show_callers || (!show_callees && !show_members) {
        let callers = ast_graph_storage::symbol_callers(conn, id)?;
        if callers.is_empty() {
            println!("├─ Callers: none");
        } else {
            // Filter out obvious false positives: callers from outside the project
            // (PDF.js, pdfjs-dist, etc.) by checking for common false-positive patterns
            let real_callers: Vec<_> = callers
                .iter()
                .filter(|c| {
                    let f = c["file_path"].as_str().unwrap_or("");
                    !f.contains("pdfjs") && !f.contains("pdf.js") && !f.contains("node_modules")
                })
                .collect();

            println!("├─ Callers ({}):", real_callers.len());
            for c in &real_callers {
                let cname = c["name"].as_str().unwrap_or("?");
                let cfile = short_path(c["file_path"].as_str().unwrap_or(""));
                let cline = c["line"].as_i64().unwrap_or(0);
                println!("│  ← {} @ {} L{}", cname, cfile, cline);
            }
        }
        println!("│");
    }

    // Callees
    if show_callees || (!show_callers && !show_members) {
        let callees = ast_graph_storage::symbol_callees(conn, id)?;
        let real_callees: Vec<_> = callees
            .iter()
            .filter(|c| {
                let f = c["file_path"].as_str().unwrap_or("");
                !f.contains("pdfjs") && !f.contains("pdf.js") && !f.contains("node_modules")
            })
            .collect();

        if real_callees.is_empty() {
            println!("└─ Calls: none");
        } else {
            println!("└─ Calls ({}):", real_callees.len());
            for c in &real_callees {
                let cname = c["name"].as_str().unwrap_or("?");
                let cfile = short_path(c["file_path"].as_str().unwrap_or(""));
                println!("   → {} [{}] @ {}", cname, c["kind"].as_str().unwrap_or("?"), cfile);
            }
        }
    }

    println!();
    Ok(())
}

fn short_path(full: &str) -> String {
    // Strip everything up to and including "Kelvin/" or "src/" for readability
    let normalized = full.replace('\\', "/");
    if let Some(pos) = normalized.find("/Kelvin/") {
        return normalized[pos + 8..].to_string();
    }
    if let Some(pos) = normalized.find("/src/") {
        return normalized[pos + 1..].to_string();
    }
    // Just take the last 3 path segments
    let parts: Vec<&str> = normalized.split('/').collect();
    if parts.len() > 3 {
        parts[parts.len() - 3..].join("/")
    } else {
        normalized
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}
