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

use std::collections::{BTreeSet, HashMap, HashSet};
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
    /// Return whether `path` is a symbolic link, without following it.
    fn is_symlink(&self, path: &Path) -> bool;
    /// The real location of `path` after resolving symlinks, when the
    /// backing store has such a notion. An in-memory tree does not.
    fn canonical(&self, _path: &Path) -> Option<PathBuf> {
        None
    }
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
    fn is_symlink(&self, path: &Path) -> bool {
        std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink())
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(path).ok()
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

    fn is_symlink(&self, _path: &Path) -> bool {
        // A flat map of file contents has no symbolic links.
        false
    }
}

/// A `Vfs` that shows only some of `inner`'s files: `visible`, and the
/// directories on the way to them. Everything else reads as absent, so
/// a walker over it sees an app made of just those files. A scoped
/// `--target fractal` build ingests through one.
pub struct ScopedVfs<'a, V: Vfs + ?Sized> {
    inner: &'a V,
    files: HashSet<PathBuf>,
    dirs: HashSet<PathBuf>,
}

impl<'a, V: Vfs + ?Sized> ScopedVfs<'a, V> {
    /// `visible` paths are compared as given, so pass them in the form
    /// the walker will ask for them (absolute under the same root).
    pub fn new(inner: &'a V, visible: impl IntoIterator<Item = PathBuf>) -> Self {
        let files: HashSet<PathBuf> = visible.into_iter().collect();
        let mut dirs = HashSet::new();
        for file in &files {
            let mut current = file.parent();
            while let Some(dir) = current {
                if !dirs.insert(dir.to_path_buf()) {
                    break;
                }
                current = dir.parent();
            }
        }
        Self { inner, files, dirs }
    }

    fn shows(&self, path: &Path) -> bool {
        self.files.contains(path) || self.dirs.contains(path)
    }

    fn absent(path: &Path) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, format!("{} is outside the scope", path.display()))
    }
}

impl<V: Vfs + ?Sized> Vfs for ScopedVfs<'_, V> {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        if !self.files.contains(path) {
            return Err(Self::absent(path));
        }
        self.inner.read(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        if !self.files.contains(path) {
            return Err(Self::absent(path));
        }
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        if !self.dirs.contains(path) {
            return Err(Self::absent(path));
        }
        Ok(self.inner.read_dir(path)?.into_iter().filter(|child| self.shows(child)).collect())
    }

    fn exists(&self, path: &Path) -> bool {
        self.shows(path) && self.inner.exists(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.dirs.contains(path) && self.inner.is_dir(path)
    }

    fn is_symlink(&self, path: &Path) -> bool {
        self.shows(path) && self.inner.is_symlink(path)
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        if self.shows(path) { self.inner.canonical(path) } else { None }
    }
}
