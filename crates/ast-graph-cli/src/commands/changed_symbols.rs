use anyhow::Result;
use ast_graph_storage::GraphStorage;
use std::collections::BTreeMap;
use std::path::Path;

use crate::git;

pub fn run(
    storage: &dyn GraphStorage,
    base: Option<&str>,
    repo_root: &Path,
    show_callers: bool,
) -> Result<()> {
    let hunks = match base {
        Some(b) => git::diff_hunks(repo_root, b)?,
        None => git::diff_hunks_worktree(repo_root)?,
    };

    if hunks.is_empty() {
        println!(
            "No diff between {} and the working tree.",
            base.unwrap_or("HEAD")
        );
        return Ok(());
    }

    let source_desc = match base {
        Some(b) => format!("{b}..HEAD"),
        None => "HEAD..worktree".to_string(),
    };

    // file_path -> symbol_id -> summary entry
    let mut by_file: BTreeMap<String, BTreeMap<String, serde_json::Value>> = BTreeMap::new();

    for hunk in &hunks {
        let rows = storage.symbols_in_range(&hunk.file_path, hunk.new_start, hunk.new_end)?;
        for row in rows {
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            let file = row["file_path"].as_str().unwrap_or(&hunk.file_path).to_string();
            by_file.entry(file).or_default().insert(id.to_string(), row);
        }
    }

    let total_symbols: usize = by_file.values().map(|m| m.len()).sum();
    if total_symbols == 0 {
        println!(
            "Diff ({}) touched {} hunk(s) but no tracked symbols overlapped.\n\
             Files may be outside the scanned language set, or changes may be in comments / whitespace.",
            source_desc,
            hunks.len(),
        );
        return Ok(());
    }

    println!(
        "Changed symbols ({}) — {} symbols across {} file(s):\n",
        source_desc,
        total_symbols,
        by_file.len(),
    );

    for (file, syms) in &by_file {
        println!("{}", file);
        // Sort symbols by start line for stable, reviewable output.
        let mut rows: Vec<&serde_json::Value> = syms.values().collect();
        rows.sort_by_key(|r| r["line_start"].as_i64().unwrap_or(0));
        for row in rows {
            let name = row["name"].as_str().unwrap_or("?");
            let kind = row["kind"].as_str().unwrap_or("?");
            let start = row["line_start"].as_i64().unwrap_or(0);
            let end = row["line_end"].as_i64().unwrap_or(0);
            println!("  L{:>5}-{:<5} {:<12} {}", start, end, kind, name);
        }
    }

    if show_callers {
        println!("\nDirect callers of each changed symbol (--callers):");
        for syms in by_file.values() {
            for row in syms.values() {
                let id = row["id"].as_str().unwrap_or("");
                let name = row["name"].as_str().unwrap_or("?");
                let callers = storage.symbol_callers(id)?;
                if callers.is_empty() {
                    println!("  {} — no callers in the graph", name);
                    continue;
                }
                println!("  {} ({} caller{}):", name, callers.len(), if callers.len() == 1 { "" } else { "s" });
                for c in callers.iter().take(10) {
                    let cname = c["name"].as_str().unwrap_or("?");
                    let cfile = c["file_path"].as_str().unwrap_or("");
                    let cline = c["call_site_line"].as_i64().unwrap_or(0);
                    println!("    ← {} @ {}:{}", cname, cfile, cline);
                }
                if callers.len() > 10 {
                    println!("    ... and {} more", callers.len() - 10);
                }
            }
        }
    }

    Ok(())
}
