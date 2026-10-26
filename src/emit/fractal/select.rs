//! Which source a fractal build compiles: the files a person names with
//! `--file`, and the `class`/`module` bodies written in them.

use std::path::Path;

use crate::App;
use crate::span::{FileId, Span};

/// The files a build compiles, and every class and module body in them.
pub struct Selection {
    /// (file, path relative to the app root), in the order named.
    pub files: Vec<(FileId, String)>,
    /// (fully qualified name, span of the whole definition) of every
    /// `class`/`module` body in those files, in file order.
    pub bodies: Vec<(String, Span)>,
}

impl Selection {
    /// The ingested sources for `files` (paths relative to `root`, the app
    /// root). A path ingest did not read is an error: a misspelt `--file`
    /// must not quietly build the identity.
    pub fn resolve(app: &App, root: &Path, files: &[String]) -> Result<Selection, String> {
        let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        let mut out: Vec<(FileId, String)> = Vec::new();
        for file in files {
            let wanted = canonical(&root.join(file));
            let found = app.sources.iter().position(|s| canonical(Path::new(&s.path)) == wanted);
            let Some(index) = found else {
                return Err(format!("--file {file}: not a Ruby source the build read"));
            };
            let id = FileId(index as u32 + 1);
            if !out.iter().any(|(f, _)| *f == id) {
                out.push((id, file.clone()));
            }
        }
        let mut bodies: Vec<(String, Span)> = app
            .const_resolver
            .for_sources(&app.sources)
            .class_bodies()
            .into_iter()
            .filter(|(_, span)| out.iter().any(|(f, _)| *f == span.file))
            .map(|(name, span)| (name.to_string(), span))
            .collect();
        bodies.sort_by_key(|(_, span)| (span.file.0, span.start));
        bodies.dedup();
        Ok(Selection { files: out, bodies })
    }

    /// The class or module whose body is the innermost one around `at`,
    /// or `None` at a file's top level.
    pub fn owner_of(&self, at: Span) -> Option<&str> {
        if at.is_synthetic() {
            return None;
        }
        self.bodies.iter().filter(|(_, s)| contains(s, at)).min_by_key(|(_, s)| s.end - s.start).map(|(name, _)| name.as_str())
    }

    /// The bodies written in `file`.
    pub fn bodies_in(&self, file: FileId) -> Vec<(String, Span)> {
        self.bodies.iter().filter(|(_, span)| span.file == file).cloned().collect()
    }
}

pub(super) fn contains(outer: &Span, inner: Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}
