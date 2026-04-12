mod commands;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "ast-graph", about = "AST Compressor + Graph Visualizer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Path to SQLite database (default: .ast-graph/graph.db in project root)
    #[arg(long, global = true)]
    db: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan a directory and build the code graph
    Scan {
        /// Path to scan
        path: String,

        /// Clear existing graph before scanning
        #[arg(long)]
        clean: bool,
    },

    /// Export the graph in various formats
    Export {
        /// Output format: json, dot, ai-context
        #[arg(short, long, default_value = "json")]
        format: String,

        /// Output file (stdout if not specified)
        #[arg(short, long)]
        output: Option<String>,

        /// Max tokens for ai-context format
        #[arg(long)]
        max_tokens: Option<usize>,
    },

    /// Run a SQL query against the graph database
    Query {
        /// SQL query string
        sql: String,
    },

    /// Show graph statistics
    Stats,

    /// Show the most connected symbols (architectural hotspots)
    Hotspots {
        /// Number of results
        #[arg(short, long, default_value = "20")]
        limit: i32,
    },

    /// Trace call chain from a function (by name)
    CallChain {
        /// Function name to trace from
        name: String,

        /// Max depth
        #[arg(short, long, default_value = "3")]
        depth: i32,
    },

    /// Look up a symbol by name — shows callers, callees, and members
    Symbol {
        /// Symbol name (partial match supported, e.g. "FinalSelection" or "TeamOnSet.save")
        name: String,

        /// Show only callers (who calls this symbol)
        #[arg(long)]
        callers: bool,

        /// Show only callees (what this symbol calls)
        #[arg(long)]
        callees: bool,

        /// Show only members (methods/properties of a class)
        #[arg(long)]
        members: bool,

        /// Max number of search results to show when name is ambiguous
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ast_graph=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Scan { path, clean } => {
            commands::scan::run(&path, cli.db.as_deref(), clean)?;
        }
        Commands::Export {
            format,
            output,
            max_tokens,
        } => {
            commands::export::run(&format, output.as_deref(), max_tokens)?;
        }
        Commands::Query { sql } => {
            commands::query::run(&sql, cli.db.as_deref())?;
        }
        Commands::Stats => {
            commands::stats::run(cli.db.as_deref())?;
        }
        Commands::Hotspots { limit } => {
            commands::hotspots::run(limit, cli.db.as_deref())?;
        }
        Commands::CallChain { name, depth } => {
            commands::call_chain::run(&name, depth, cli.db.as_deref())?;
        }
        Commands::Symbol { name, callers, callees, members, limit } => {
            commands::symbol::run(&name, callers, callees, members, limit, cli.db.as_deref())?;
        }
    }

    Ok(())
}
