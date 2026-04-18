use anyhow::Result;
use ast_graph_storage::GraphStorage;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::git;

pub fn run(
    name: &str,
    depth: i32,
    storage: &dyn GraphStorage,
    with_recency: bool,
    recency_days: u32,
    repo_root: &Path,
) -> Result<()> {
    // Resolve the target symbol to a single node (same disambiguation pattern
    // as call-chain).
    let matches = storage.find_symbols(name, 10)?;
    let candidates: Vec<&serde_json::Value> = matches
        .iter()
        .filter(|m| {
            let k = m["kind"].as_str().unwrap_or("");
            matches!(
                k,
                "Function" | "Method" | "Constructor" | "Class" | "Interface" | "Trait" | "Struct"
            )
        })
        .collect();

    if candidates.is_empty() {
        println!(
            "No symbol matching '{}' found. Run 'ast-graph scan .' first.",
            name
        );
        return Ok(());
    }

    if candidates.len() > 1 {
        println!("Multiple matches found — showing blast radius for the first:");
        for r in &candidates {
            println!(
                "  {} ({})",
                r["name"].as_str().unwrap_or("?"),
                r["kind"].as_str().unwrap_or("?"),
            );
        }
        println!();
    }

    let node_id = candidates[0]["id"].as_str().unwrap_or("");
    let node_name = candidates[0]["name"].as_str().unwrap_or(name);

    let upstream = storage.reverse_call_chain(node_id, depth)?;
    if upstream.is_empty() {
        println!(
            "'{}' has no inbound CALLS edges — nothing breaks if you change it (as far as the graph knows).",
            node_name
        );
        return Ok(());
    }

    // Group by depth for readable output.
    let mut by_depth: BTreeMap<i64, Vec<&serde_json::Value>> = BTreeMap::new();
    let mut unique_callers: HashSet<String> = HashSet::new();
    for entry in &upstream {
        let d = entry["depth"].as_i64().unwrap_or(0);
        by_depth.entry(d).or_default().push(entry);
        if let Some(id) = entry["id"].as_str() {
            unique_callers.insert(id.to_string());
        }
    }

    println!(
        "Blast radius for '{}' — {} distinct callers within {} hop(s):\n",
        node_name,
        unique_callers.len(),
        depth
    );

    // Optionally fetch churn for every involved file, once.
    let churn_map = if with_recency {
        let files: Vec<String> = upstream
            .iter()
            .filter_map(|e| e["file_path"].as_str().map(str::to_string))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        match git::file_churn(repo_root, &files, recency_days) {
            Ok(m) => Some(m),
            Err(e) => {
                eprintln!(
                    "warn: --with-recency requested but `git log` failed ({e}); \
                     showing blast radius without churn."
                );
                None
            }
        }
    } else {
        None
    };

    for (d, callers) in &by_depth {
        println!("L{} (depth {}), {} symbol(s):", d, d, callers.len());
        for c in callers {
            let name = c["name"].as_str().unwrap_or("?");
            let kind = c["kind"].as_str().unwrap_or("?");
            let file = c["file_path"].as_str().unwrap_or("");
            let call_line = c["call_line"].as_i64().unwrap_or(0);
            let line_tag = if call_line > 0 {
                format!(":{}", call_line)
            } else {
                String::new()
            };
            let recency_tag = churn_map
                .as_ref()
                .and_then(|m| m.get(file))
                .map(|ch| {
                    format!(
                        "  [churn: {} commits in {}d, last {}]",
                        ch.commits_in_window,
                        recency_days,
                        git::humanize_ago(ch.last_commit_ts),
                    )
                })
                .unwrap_or_default();
            println!(
                "  ← {:<40} {:<14} @ {}{}{}",
                name, kind, file, line_tag, recency_tag,
            );
        }
        println!();
    }

    // Summary: most-affected files by caller count.
    let mut files_count: BTreeMap<String, usize> = BTreeMap::new();
    for c in &upstream {
        if let Some(f) = c["file_path"].as_str() {
            *files_count.entry(f.to_string()).or_default() += 1;
        }
    }
    let mut ranked: Vec<(String, usize)> = files_count.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1));

    println!("Most-affected files:");
    for (file, count) in ranked.iter().take(10) {
        println!("  {:>4}  {}", count, file);
    }

    if with_recency && churn_map.is_some() {
        println!();
        println!(
            "Recency is measured over the last {} days. Hot-and-connected symbols are the ones \
             worth double-checking before you change the target.",
            recency_days
        );
    }

    Ok(())
}
