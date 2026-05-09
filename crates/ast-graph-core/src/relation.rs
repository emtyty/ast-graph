use serde::{Deserialize, Serialize};
use std::fmt;

use crate::symbol::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeKind {
    Contains,
    Calls,
    Imports,
    Extends,
    Implements,
    References,
    OverridesMethod,
    /// A symbol (controller method, function) handles an HTTP route node.
    HandlesRoute,
    /// A symbol participates in a process / execution flow at a given step.
    /// `source_line` is repurposed as the step index (1-based) on these edges.
    StepInProcess,
    /// A symbol is the entry point of a process.
    EntryPointOf,
    /// A symbol calls an HTTP endpoint by URL — connects client-side
    /// `fetch('/api/x')` / `axios.get('/api/x')` / `useSWR('/api/x')` to
    /// a `Route` node with matching `<METHOD> <path>` name. Lets `blast-radius`
    /// and `call-chain` cross the HTTP boundary in a full-stack repo.
    Fetches,
}

impl EdgeKind {
    pub fn as_neo4j_type(&self) -> &'static str {
        match self {
            Self::Contains => "CONTAINS",
            Self::Calls => "CALLS",
            Self::Imports => "IMPORTS",
            Self::Extends => "EXTENDS",
            Self::Implements => "IMPLEMENTS",
            Self::References => "REFERENCES",
            Self::OverridesMethod => "OVERRIDES",
            Self::HandlesRoute => "HANDLES_ROUTE",
            Self::StepInProcess => "STEP_IN_PROCESS",
            Self::EntryPointOf => "ENTRY_POINT_OF",
            Self::Fetches => "FETCHES",
        }
    }

    pub fn from_neo4j_type(s: &str) -> Option<Self> {
        match s {
            "CONTAINS" => Some(Self::Contains),
            "CALLS" => Some(Self::Calls),
            "IMPORTS" => Some(Self::Imports),
            "EXTENDS" => Some(Self::Extends),
            "IMPLEMENTS" => Some(Self::Implements),
            "REFERENCES" => Some(Self::References),
            "OVERRIDES" => Some(Self::OverridesMethod),
            "HANDLES_ROUTE" => Some(Self::HandlesRoute),
            "STEP_IN_PROCESS" => Some(Self::StepInProcess),
            "ENTRY_POINT_OF" => Some(Self::EntryPointOf),
            "FETCHES" => Some(Self::Fetches),
            _ => None,
        }
    }

    /// Every variant, for iteration (e.g. per-kind batching in Cypher).
    pub const ALL: &'static [EdgeKind] = &[
        EdgeKind::Contains,
        EdgeKind::Calls,
        EdgeKind::Imports,
        EdgeKind::Extends,
        EdgeKind::Implements,
        EdgeKind::References,
        EdgeKind::OverridesMethod,
        EdgeKind::HandlesRoute,
        EdgeKind::StepInProcess,
        EdgeKind::EntryPointOf,
        EdgeKind::Fetches,
    ];
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_neo4j_type())
    }
}

/// Resolution confidence tier attached to every edge. Higher = more certain
/// the target is the correct definition.
///
/// - `1.0`  — exact / structural (parent→child CONTAINS, by-path import
///            resolution, signature-disambiguated overload)
/// - `0.95` — same-file name match (no ambiguity within the file)
/// - `0.9`  — import-scoped name match (target's file is in caller's import set)
/// - `0.5`  — global name fallback (last-segment / partial-qualified guess)
pub const CONFIDENCE_EXACT: f32 = 1.0;
pub const CONFIDENCE_SAME_FILE: f32 = 0.95;
pub const CONFIDENCE_IMPORT_SCOPED: f32 = 0.9;
pub const CONFIDENCE_GLOBAL: f32 = 0.5;

/// A resolved edge between two known nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub kind: EdgeKind,
    /// Line in the source file where this edge originates (e.g. the call site
    /// for a CALLS edge, the `use` statement for IMPORTS). Zero for edges
    /// that have no meaningful line (e.g. structural CONTAINS edges).
    pub source_line: u32,
    /// Resolution confidence. See `CONFIDENCE_*` constants.
    /// Defaults to `CONFIDENCE_EXACT` for edges built before this field
    /// was tagged (loaded from old DBs, structural edges).
    #[serde(default = "default_confidence")]
    pub confidence: f32,
}

fn default_confidence() -> f32 {
    CONFIDENCE_EXACT
}

/// An unresolved edge with a string-based target (pre-resolution).
/// Created during AST extraction, resolved to `Edge` in the resolution phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEdge {
    pub source: NodeId,
    pub kind: EdgeKind,
    pub target_name: String,
    pub target_module: Option<String>,
    pub source_line: u32,
}
