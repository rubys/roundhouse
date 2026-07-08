//! Per-file Go import accumulator for the go2 emitter.
//!
//! Today the emitted `package v2` overlay puts every file in one
//! package, so the import-detection scan in
//! `go2::rewrite_package_to_v2` could get away with a content-substring
//! check against a small fixed table of stdlib packages. That doesn't
//! survive the Phase 4 (#19) cutover, where each file lives in its own
//! Go package and imports sibling packages by path
//! (`github.com/<user>/<app>/internal/models`, etc.) — those names
//! aren't fixed and aren't safe to grep for in file bodies.
//!
//! `FileImports` is the surface that Phase 3 emit code will call
//! `add` on when it produces a cross-package reference. Phase 1.2
//! introduces the type and threads it through `rewrite_package_to_v2`
//! so the API shape is in place before Phase 3 starts populating it.
//! Behavior is unchanged: callers pass `FileImports::new()` and the
//! existing content-scan continues to supply the stdlib imports.

use std::collections::BTreeSet;

/// Set of Go import paths needed by a single emitted file. Phase 1.2
/// callers leave this empty (the content-scan fallback supplies the
/// stdlib paths). Phase 3+ emit code calls `add` whenever it produces
/// a reference to a sibling package.
#[derive(Default, Clone)]
pub(crate) struct FileImports {
    /// Set of Go import paths. Stored as `String` (not `&'static str`)
    /// because sibling-package paths are runtime-computed
    /// (e.g. `"app/internal/models"`).
    paths: BTreeSet<String>,
}

impl FileImports {
    /// Create an empty import set.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register a Go import path. Idempotent; duplicates are collapsed
    /// by the underlying set.
    #[allow(dead_code)]
    pub(crate) fn add(&mut self, path: impl Into<String>) {
        self.paths.insert(path.into());
    }

    /// Iterate the registered import paths in alphabetical order (Go
    /// convention — `gofmt` sorts imports this way).
    pub(crate) fn iter(&self) -> impl Iterator<Item = &str> {
        self.paths.iter().map(String::as_str)
    }

    /// True iff no imports have been registered. (Test-only for now.)
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.paths.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_by_default() {
        let imports = FileImports::new();
        assert!(imports.is_empty());
        assert_eq!(imports.iter().count(), 0);
    }

    #[test]
    fn add_dedupes_and_sorts() {
        let mut imports = FileImports::new();
        imports.add("fmt");
        imports.add("strings");
        imports.add("fmt");
        imports.add("encoding/json");
        let collected: Vec<&str> = imports.iter().collect();
        assert_eq!(collected, vec!["encoding/json", "fmt", "strings"]);
    }
}
