use anyhow::Result;
use std::path::Path;

use ast_graph_core::*;

pub fn run(
    format: &str,
    output: Option<&str>,
    max_tokens: Option<usize>,
) -> Result<()> {
    let mut graph = ast_graph_parse::parse_project(Path::new("."))?;
    ast_graph_resolve::resolve_edges(&mut graph, Path::new("."));

    let content = match format {
        "json" => export_json(&graph)?,
        "dot" => export_dot(&graph),
        "ai-context" => export_ai_context(&graph, max_tokens),
        _ => anyhow::bail!("Unknown format: {}. Use: json, dot, ai-context", format),
    };

    match output {
        Some(path) => {
            std::fs::write(path, &content)?;
            println!("Exported to {path}");
        }
        None => {
            print!("{content}");
        }
    }

    Ok(())
}

fn export_json(graph: &CodeGraph) -> Result<String> {
    Ok(serde_json::to_string_pretty(graph)?)
}

fn export_dot(graph: &CodeGraph) -> String {
    let mut dot = String::from("digraph CodeGraph {\n  rankdir=LR;\n  node [shape=box fontsize=10];\n\n");

    for (id, node) in &graph.nodes {
        if node.kind == SymbolKind::Import || node.kind == SymbolKind::File {
            continue;
        }
        let color = match node.kind {
            SymbolKind::Function | SymbolKind::Method => "lightblue",
            SymbolKind::Class | SymbolKind::Struct | SymbolKind::Record => "lightyellow",
            SymbolKind::Trait | SymbolKind::Interface => "lightgreen",
            SymbolKind::Enum => "lightsalmon",
            _ => "white",
        };
        let label = if node.name.contains("::") {
            node.name.split("::").last().unwrap_or(&node.name)
        } else {
            &node.name
        };
        dot.push_str(&format!(
            "  \"{}\" [label=\"{}\" style=filled fillcolor=\"{}\"];\n",
            id, label, color
        ));
    }

    dot.push('\n');

    for edges in graph.adjacency.values() {
        for edge in edges {
            if edge.kind == EdgeKind::Contains {
                continue;
            }
            dot.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                edge.source, edge.target, edge.kind
            ));
        }
    }

    dot.push_str("}\n");
    dot
}

fn export_ai_context(graph: &CodeGraph, max_tokens: Option<usize>) -> String {
    let mut output = String::new();
    let max_chars = max_tokens.map(|t| t * 4);

    let langs: Vec<_> = graph.metadata.languages.iter().map(|l| l.as_str()).collect();
    output.push_str(&format!(
        "# Project ({}) — {} symbols, {} relationships\n\n",
        langs.join(", "),
        graph.metadata.total_nodes,
        graph.metadata.total_edges,
    ));

    let mut file_groups: Vec<(&std::path::PathBuf, &Vec<NodeId>)> =
        graph.file_index.iter().collect();
    file_groups.sort_by_key(|(path, _)| path.to_string_lossy().to_string());

    for (file_path, node_ids) in file_groups {
        let relative = file_path
            .strip_prefix(&graph.project_root)
            .unwrap_or(file_path);

        let mut file_section = format!("## {}\n", relative.display());

        let mut nodes: Vec<&SymbolNode> = node_ids
            .iter()
            .filter_map(|id| graph.nodes.get(id))
            .filter(|n| n.kind != SymbolKind::File && n.kind != SymbolKind::Import)
            .collect();
        nodes.sort_by_key(|n| match n.kind {
            SymbolKind::Function | SymbolKind::Method => 0,
            SymbolKind::Class | SymbolKind::Struct | SymbolKind::Trait => 1,
            _ => 2,
        });

        for node in nodes {
            let sig = node.signature.as_deref().unwrap_or(&node.name);
            file_section.push_str(&format!(
                "  {}  [L{}-L{}]\n",
                sig, node.line_range.0, node.line_range.1
            ));

            let calls: Vec<_> = graph
                .outgoing(&node.id)
                .iter()
                .filter(|e| e.kind == EdgeKind::Calls)
                .filter_map(|e| graph.nodes.get(&e.target))
                .map(|n| n.name.as_str())
                .collect();
            if !calls.is_empty() {
                file_section.push_str(&format!("    calls: {}\n", calls.join(", ")));
            }

            let called_by: Vec<_> = graph
                .incoming(&node.id)
                .iter()
                .filter(|e| e.kind == EdgeKind::Calls)
                .filter_map(|e| graph.nodes.get(&e.target))
                .map(|n| n.name.as_str())
                .collect();
            if !called_by.is_empty() {
                file_section.push_str(&format!("    called_by: {}\n", called_by.join(", ")));
            }
        }

        file_section.push('\n');

        if let Some(max) = max_chars {
            if output.len() + file_section.len() > max {
                output.push_str("... (truncated)\n");
                break;
            }
        }

        output.push_str(&file_section);
    }

    output
}
