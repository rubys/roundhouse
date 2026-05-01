//! Virtual filesystem abstraction for ingest.
//!
//! `ingest_app` historically read the source tree directly from disk via
//! `std::fs`. For the in-browser transpile use case (roundhouse compiled to
//! wasm), the source tree arrives as JSON and lives in memory. This trait
//! lets the same ingest pipeline drive both shapes.
//!
//! The trait surface is intentionally tiny — only the five operations the
//! ingester actually needs.
//!
//! Directories are derived from file paths: a path is "a directory" iff
//! some other path starts with it. There's no separate directory entry.

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::{Path, PathBuf};

/// Operations the ingester needs from the underlying source tree.
pub trait Vfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>>;
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    /// Return immediate children (files and dirs) of `path`. Order is unspecified.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
}

/// Real-filesystem-backed `Vfs`. Used by the CLI and tests.
pub struct FsVfs;

impl FsVfs {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FsVfs {
    fn default() -> Self {
        Self::new()
    }
}

impl Vfs for FsVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            out.push(entry?.path());
        }
        Ok(out)
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}

/// In-memory `Vfs` backed by a flat path → bytes map. Directories
/// are inferred from path prefixes; there are no explicit directory
/// entries. Used by the wasm transpile entry point.
pub struct MapVfs {
    files: HashMap<PathBuf, Vec<u8>>,
}

impl MapVfs {
    pub fn new(files: HashMap<PathBuf, Vec<u8>>) -> Self {
        Self { files }
    }
}

impl Vfs for MapVfs {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.files
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{path:?}")))
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        let bytes = self.read(path)?;
        String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        // An entry is an immediate child of `path` if `path` is its
        // direct parent (file case) or if some file lives at
        // `path/segment/...` for some segment (subdir case).
        let mut out: BTreeSet<PathBuf> = BTreeSet::new();
        for file in self.files.keys() {
            let Ok(rel) = file.strip_prefix(path) else {
                continue;
            };
            let mut components = rel.components();
            let Some(first) = components.next() else {
                continue;
            };
            // If there are more components, it's a subdir; otherwise
            // it's a direct file child.
            let child = path.join(first.as_os_str());
            out.insert(child);
        }
        Ok(out.into_iter().collect())
    }

    fn exists(&self, path: &Path) -> bool {
        if self.files.contains_key(path) {
            return true;
        }
        // Inferred directory: some file lives under it.
        self.files.keys().any(|f| f.starts_with(path))
    }

    fn is_dir(&self, path: &Path) -> bool {
        // It's a directory iff it's not itself a file but some file
        // lives strictly underneath it.
        if self.files.contains_key(path) {
            return false;
        }
        self.files
            .keys()
            .any(|f| f.starts_with(path) && f != path)
    }
}
