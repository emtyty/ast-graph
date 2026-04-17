mod commands;

use anyhow::Result;
use ast_graph_storage::GraphStorage;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum BackendArg {
    Sqlite,
    Falkor,
}

#[derive(Parser)]
#[command(name = "ast-graph", about = "AST Compressor + Graph Visualizer")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Which database backend to use.
    #[arg(long, value_enum, default_value_t = BackendArg::Sqlite, global = true)]
    backend: BackendArg,

    /// Path to SQLite database (default: .ast-graph/graph.db in project root).
    /// Only used when --backend=sqlite.
    #[arg(long, global = true)]
    db: Option<PathBuf>,

    /// FalkorDB connection URL (e.g. "falkor://127.0.0.1:6379").
    /// Only used when --backend=falkor.
    #[arg(
        long,
        global = true,
        env = "FALKOR_URL",
        default_value = "falkor://127.0.0.1:6379"
    )]
    falkor_url: String,

    /// FalkorDB graph name.
    #[arg(long, global = true, default_value = "code_graph")]
    falkor_graph_name: String,
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

    /// Run a backend-native query (SQL for SQLite, Cypher for FalkorDB)
    Query {
        /// Query string
        query: String,
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

/// Build the chosen backend. For SQLite we resolve the default path against
/// the scan target when available, otherwise against the current directory.
fn build_storage(cli: &Cli, fallback_root: &Path) -> Result<Box<dyn GraphStorage>> {
    match cli.backend {
        BackendArg::Sqlite => {
            let db_file = cli
                .db
                .clone()
                .unwrap_or_else(|| ast_graph_storage::default_db_path(fallback_root));
            ast_graph_storage::open_sqlite(&db_file)
        }
        BackendArg::Falkor => {
            let cfg = ast_graph_storage::FalkorConfig {
                url: cli.falkor_url.clone(),
                graph_name: cli.falkor_graph_name.clone(),
            };
            ast_graph_storage::open_falkor(cfg)
        }
    }
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ast_graph=info".into()),
        )
        .init();

    let cli = Cli::parse();

    // Export doesn't touch storage — it's a pure parse+resolve pipeline.
    if let Commands::Export {
        format,
        output,
        max_tokens,
    } = &cli.command
    {
        return commands::export::run(format, output.as_deref(), *max_tokens);
    }

    // Resolve the default SQLite path against the scan target when available.
    let fallback_root = match &cli.command {
        Commands::Scan { path, .. } => Path::new(path).canonicalize()?,
        _ => Path::new(".").canonicalize()?,
    };
    let storage = build_storage(&cli, &fallback_root)?;

    match cli.command {
        Commands::Scan { path, clean } => {
            commands::scan::run(&path, storage.as_ref(), clean)?;
        }
        Commands::Export { .. } => unreachable!("handled above"),
        Commands::Query { query } => {
            commands::query::run(&query, storage.as_ref())?;
        }
        Commands::Stats => {
            commands::stats::run(storage.as_ref())?;
        }
        Commands::Hotspots { limit } => {
            commands::hotspots::run(limit, storage.as_ref())?;
        }
        Commands::CallChain { name, depth } => {
            commands::call_chain::run(&name, depth, storage.as_ref())?;
        }
        Commands::Symbol {
            name,
            callers,
            callees,
            members,
            limit,
        } => {
            commands::symbol::run(&name, callers, callees, members, limit, storage.as_ref())?;
        }
    }

    Ok(())
}
