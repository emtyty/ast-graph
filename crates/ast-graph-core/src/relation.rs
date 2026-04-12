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
            _ => None,
        }
    }
}

impl fmt::Display for EdgeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_neo4j_type())
    }
}

/// A resolved edge between two known nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: NodeId,
    pub target: NodeId,
    pub kind: EdgeKind,
}

/// An unresolved edge with a string-based target (pre-resolution).
/// Created during AST extraction, resolved to `Edge` in the resolution phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawEdge {
    pub source: NodeId,
    pub kind: EdgeKind,
    pub target_name: String,
    pub target_module: Option<String>,
}
