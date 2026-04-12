use serde::{Deserialize, Serialize};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Stable identifier for a symbol node, derived from hashing
/// (file_path, name, kind, line_start) for consistency across incremental runs.
#[derive(Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u64);

impl NodeId {
    pub fn new(file_path: &str, name: &str, kind: SymbolKind, line_start: u32) -> Self {
        use std::hash::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        file_path.hash(&mut hasher);
        name.hash(&mut hasher);
        kind.hash(&mut hasher);
        line_start.hash(&mut hasher);
        NodeId(hasher.finish())
    }

    pub fn from_hex(s: &str) -> Option<Self> {
        u64::from_str_radix(s, 16).ok().map(NodeId)
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({:016x})", self.0)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    File,
    Module,
    Package,
    Function,
    Method,
    Constructor,
    Class,
    Struct,
    Enum,
    Union,
    Interface,
    Trait,
    TypeAlias,
    Constant,
    Static,
    Import,
    Field,
    EnumVariant,
    Property,
    Namespace,
    Record,
}

impl SymbolKind {
    pub fn as_neo4j_label(&self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Module => "Module",
            Self::Package => "Package",
            Self::Function => "Function",
            Self::Method => "Method",
            Self::Constructor => "Constructor",
            Self::Class => "Class",
            Self::Struct => "Struct",
            Self::Enum => "Enum",
            Self::Union => "Union",
            Self::Interface => "Interface",
            Self::Trait => "Trait",
            Self::TypeAlias => "TypeAlias",
            Self::Constant => "Constant",
            Self::Static => "Static",
            Self::Import => "Import",
            Self::Field => "Field",
            Self::EnumVariant => "EnumVariant",
            Self::Property => "Property",
            Self::Namespace => "Namespace",
            Self::Record => "Record",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        match s {
            "File" => Some(Self::File),
            "Module" => Some(Self::Module),
            "Package" => Some(Self::Package),
            "Function" => Some(Self::Function),
            "Method" => Some(Self::Method),
            "Constructor" => Some(Self::Constructor),
            "Class" => Some(Self::Class),
            "Struct" => Some(Self::Struct),
            "Enum" => Some(Self::Enum),
            "Union" => Some(Self::Union),
            "Interface" => Some(Self::Interface),
            "Trait" => Some(Self::Trait),
            "TypeAlias" => Some(Self::TypeAlias),
            "Constant" => Some(Self::Constant),
            "Static" => Some(Self::Static),
            "Import" => Some(Self::Import),
            "Field" => Some(Self::Field),
            "EnumVariant" => Some(Self::EnumVariant),
            "Property" => Some(Self::Property),
            "Namespace" => Some(Self::Namespace),
            "Record" => Some(Self::Record),
            _ => None,
        }
    }
}

impl fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_neo4j_label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private,
    Protected,
    Internal,
}

impl Visibility {
    pub fn from_debug_str(s: &str) -> Self {
        match s {
            "Public" => Self::Public,
            "Protected" => Self::Protected,
            "Internal" => Self::Internal,
            _ => Self::Private,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    CSharp,
}

impl Language {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "ts" | "tsx" => Some(Self::TypeScript),
            "cs" => Some(Self::CSharp),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::CSharp => "csharp",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "rust" => Some(Self::Rust),
            "python" => Some(Self::Python),
            "javascript" => Some(Self::JavaScript),
            "typescript" => Some(Self::TypeScript),
            "csharp" => Some(Self::CSharp),
            _ => None,
        }
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A compressed symbol extracted from the AST.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolNode {
    pub id: NodeId,
    pub name: String,
    pub kind: SymbolKind,
    pub file_path: PathBuf,
    pub line_range: (u32, u32),
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub visibility: Visibility,
    pub language: Language,
    pub parent: Option<NodeId>,
}
