use ast_graph_core::{hash_bytes, Language};
use ignore::WalkBuilder;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};

/// Discovered source file with its language and content hash.
#[derive(Debug, Clone)]
pub struct DiscoveredFile {
    pub path: PathBuf,
    pub language: Language,
    pub hash: [u8; 32],
}

/// Walk a directory, respecting .gitignore, and discover all supported source files.
pub fn discover_files(root: &Path) -> Vec<DiscoveredFile> {
    let mut files = Vec::new();

    let walker = WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();

    for entry in walker.flatten() {
        if !entry.file_type().map_or(false, |ft| ft.is_file()) {
            continue;
        }

        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

        if let Some(language) = Language::from_extension(ext) {
            if let Ok(contents) = std::fs::read(path) {
                let hash = hash_bytes(&contents);
                files.push(DiscoveredFile {
                    path: path.to_path_buf(),
                    language,
                    hash,
                });
            }
        }
    }

    files
}

/// Build a hash map from discovered files for incremental comparison.
pub fn build_hash_map(files: &[DiscoveredFile]) -> FxHashMap<PathBuf, [u8; 32]> {
    files.iter().map(|f| (f.path.clone(), f.hash)).collect()
}
