use rustc_hash::FxHashMap;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Tracks file hashes for incremental parsing.
#[derive(Debug, Clone, Default)]
pub struct IncrementalState {
    pub file_hashes: FxHashMap<PathBuf, [u8; 32]>,
}

pub struct FileChanges {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub removed: Vec<PathBuf>,
}

impl IncrementalState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_hashes(hashes: FxHashMap<PathBuf, [u8; 32]>) -> Self {
        Self { file_hashes: hashes }
    }

    /// Compare current file hashes against stored state.
    pub fn detect_changes(&self, current: &FxHashMap<PathBuf, [u8; 32]>) -> FileChanges {
        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut removed = Vec::new();

        for (path, hash) in current {
            match self.file_hashes.get(path) {
                None => added.push(path.clone()),
                Some(old_hash) if old_hash != hash => modified.push(path.clone()),
                _ => {}
            }
        }

        for path in self.file_hashes.keys() {
            if !current.contains_key(path) {
                removed.push(path.clone());
            }
        }

        FileChanges {
            added,
            modified,
            removed,
        }
    }
}

/// Compute SHA-256 hash of file contents.
pub fn hash_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}
