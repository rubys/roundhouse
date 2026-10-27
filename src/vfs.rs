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

use std::cell::RefCell;
use std::collections::BTreeSet;
use crate::det::HashMap;
use std::io;
use std::path::{Path, PathBuf};

/// In-memory path → bytes tree handed to [`MapVfs`] / `ingest_app_from_tree`.
///
/// Stays on std's `RandomState` hasher: the tree is public input that is only
/// looked up, never walked for emit/fixpoint order, and tests/wasm build it
/// with std. Library maps use [`crate::det::HashMap`] instead.
pub type FileTree = std::collections::HashMap<PathBuf, Vec<u8>>;

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
    files: FileTree,
}

impl MapVfs {
    pub fn new(files: FileTree) -> Self {
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

/// A `Vfs` that remembers what `inner` said about a path's existence, kind
/// and link-ness. One ingest reads a tree that does not change under it,
/// and its walkers ask the same question of the same path again and again
/// (a `require` probed against every load path, each prefix of a path
/// checked for links): answered from the first lookup, each costs a hash
/// instead of a system call. Nothing below a path that is absent is asked
/// of `inner`. Reads and listings pass through.
pub struct CachedVfs<'a, V: Vfs + ?Sized> {
    inner: &'a V,
    facts: RefCell<HashMap<PathBuf, Facts>>,
}

#[derive(Clone, Copy, Default)]
struct Facts {
    exists: Option<bool>,
    dir: Option<bool>,
    symlink: Option<bool>,
}

impl<'a, V: Vfs + ?Sized> CachedVfs<'a, V> {
    pub fn new(inner: &'a V) -> Self {
        Self { inner, facts: RefCell::new(HashMap::default()) }
    }

    fn fact(
        &self,
        path: &Path,
        read: impl Fn(&Facts) -> Option<bool>,
        ask: impl FnOnce() -> bool,
        write: impl FnOnce(&mut Facts, bool),
    ) -> bool {
        if let Some(known) = self.facts.borrow().get(path).and_then(&read) {
            return known;
        }
        let answer = ask();
        write(self.facts.borrow_mut().entry(path.to_path_buf()).or_default(), answer);
        answer
    }
}

impl<V: Vfs + ?Sized> Vfs for CachedVfs<'_, V> {
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        self.inner.read_dir(path)
    }

    fn exists(&self, path: &Path) -> bool {
        self.fact(
            path,
            |f| f.exists,
            || {
                let parent = path.parent().filter(|parent| !parent.as_os_str().is_empty());
                parent.is_none_or(|parent| self.exists(parent)) && self.inner.exists(path)
            },
            |f, v| f.exists = Some(v),
        )
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.fact(path, |f| f.dir, || self.inner.is_dir(path), |f, v| f.dir = Some(v))
    }

    fn is_symlink(&self, path: &Path) -> bool {
        self.fact(path, |f| f.symlink, || self.inner.is_symlink(path), |f, v| f.symlink = Some(v))
    }

    fn canonical(&self, path: &Path) -> Option<PathBuf> {
        self.inner.canonical(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A tree that counts how often each probe reaches it.
    struct Counting {
        inner: MapVfs,
        exists: Cell<usize>,
        is_dir: Cell<usize>,
    }

    impl Vfs for Counting {
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.inner.read(path)
        }
        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            self.inner.read_to_string(path)
        }
        fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
            self.inner.read_dir(path)
        }
        fn exists(&self, path: &Path) -> bool {
            self.exists.set(self.exists.get() + 1);
            self.inner.exists(path)
        }
        fn is_dir(&self, path: &Path) -> bool {
            self.is_dir.set(self.is_dir.get() + 1);
            self.inner.is_dir(path)
        }
        fn is_symlink(&self, path: &Path) -> bool {
            self.inner.is_symlink(path)
        }
    }

    fn counting() -> Counting {
        // MapVfs input is FileTree (std); CachedVfs::facts uses det.
        let files = FileTree::from_iter([(PathBuf::from("/app/lib/a.rb"), b"A = 1".to_vec())]);
        Counting { inner: MapVfs::new(files), exists: Cell::new(0), is_dir: Cell::new(0) }
    }

    #[test]
    fn cached_vfs_answers_as_the_tree_does_and_asks_it_once() {
        let tree = counting();
        let cached = CachedVfs::new(&tree);
        for _ in 0..3 {
            for path in ["/app/lib/a.rb", "/app/lib", "/app/lib/missing.rb", "/nowhere/else.rb"] {
                let path = Path::new(path);
                assert_eq!(cached.exists(path), tree.inner.exists(path), "{}", path.display());
                assert_eq!(cached.is_dir(path), tree.inner.is_dir(path), "{}", path.display());
            }
        }
        // 4 paths asked 3 times each reached the tree at most once per path
        // (and ancestors), not 12 times.
        assert!(tree.is_dir.get() <= 4, "is_dir reached the tree {} times", tree.is_dir.get());
        assert!(tree.exists.get() <= 4 * 4, "exists reached the tree {} times", tree.exists.get());
        let before = tree.exists.get();
        assert!(!cached.exists(Path::new("/nowhere/else.rb")));
        assert_eq!(tree.exists.get(), before, "a repeated probe is answered from the memo");
    }

    #[test]
    fn cached_vfs_does_not_ask_below_an_absent_parent() {
        let tree = counting();
        let cached = CachedVfs::new(&tree);
        assert!(!cached.exists(Path::new("/gone")));
        let before = tree.exists.get();
        assert!(!cached.exists(Path::new("/gone/deep/er/file.rb")));
        // `/gone/deep/er/file.rb`, `/gone/deep/er`, `/gone/deep` are answered
        // by walking up to the memoized absent `/gone`: the tree is not asked.
        assert_eq!(tree.exists.get(), before);
    }

    #[test]
    fn cached_vfs_passes_reads_and_listings_through() {
        let tree = counting();
        let cached = CachedVfs::new(&tree);
        assert_eq!(cached.read_to_string(Path::new("/app/lib/a.rb")).unwrap(), "A = 1");
        assert_eq!(cached.read_dir(Path::new("/app/lib")).unwrap(), vec![PathBuf::from("/app/lib/a.rb")]);
        assert!(cached.read(Path::new("/app/lib/b.rb")).is_err());
    }
}
