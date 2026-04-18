use anyhow::Result;
use ast_graph_storage::GraphStorage;

/// Default exclusion substrings — vendored / generated files that produce noise.
const DEFAULT_EXCLUDES: &[&str] = &[
    "node_modules",
    "dist/",
    "build/",
    "target/",
    ".min.js",
    ".min.mjs",
    ".bundle.js",
    "pdf.worker",
    "vendor/",
    "third_party/",
];

pub fn run(
    storage: &dyn GraphStorage,
    limit: i32,
    kinds: Option<&str>,
    include_all: bool,
) -> Result<()> {
    let parsed_kinds: Vec<String> = match kinds {
        Some(s) => s.split(',').map(|k| k.trim().to_string()).collect(),
        None => vec!["Function".into(), "Method".into(), "Constructor".into()],
    };
    let kind_refs: Vec<&str> = parsed_kinds.iter().map(|s| s.as_str()).collect();

    let excludes: &[&str] = if include_all { &[] } else { DEFAULT_EXCLUDES };

    let results = storage.dead_symbols(&kind_refs, excludes, limit)?;

    if results.is_empty() {
        println!("No dead {} found.", parsed_kinds.join("/"));
        println!("(A symbol is 'dead' if the graph contains zero incoming CALLS edges to it.");
        println!(" Entry points and dynamically-dispatched methods will show up here too — review results before deleting.)");
        return Ok(());
    }

    println!(
        "Found {} {} with no inbound CALLS edges (likely dead):\n",
        results.len(),
        parsed_kinds.join("/"),
    );

    let mut last_file = String::new();
    for entry in &results {
        let file = entry["file_path"].as_str().unwrap_or("?");
        if file != last_file {
            println!("{}", file);
            last_file = file.to_string();
        }
        let name = entry["name"].as_str().unwrap_or("?");
        let kind = entry["kind"].as_str().unwrap_or("?");
        let line = entry["line_start"].as_i64().unwrap_or(0);
        let visibility = entry["visibility"].as_str().unwrap_or("");
        let vis_tag = if visibility.is_empty() {
            String::new()
        } else {
            format!(" [{}]", visibility)
        };
        println!("  L{:>5}  {:<12} {}{}", line, kind, name, vis_tag);
    }

    println!();
    println!("Caveats:");
    println!("  - Entry points (main, HTTP handlers, event listeners) have no callers by design.");
    println!("  - Dynamic dispatch (virtual calls, reflection, callbacks) isn't visible to the graph.");
    println!("  - Public library APIs may be called by external consumers.");
    println!("  - Run with --include-all to disable vendored-file exclusions.");

    Ok(())
}
