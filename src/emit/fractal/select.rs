//! Which source a fractal build compiles: the files a person names with
//! `--file`, and the `class`/`module` bodies written in them.

use std::path::Path;

use crate::App;
use crate::analyze::{ConstResolver, has_ambiguous_namespace};
use crate::span::{FileId, SourceFile, Span};

use super::pass::Context;

/// The files a build compiles, and every class and module body in them.
pub struct Selection {
    /// (file, path relative to the app root), in the order named.
    pub files: Vec<(FileId, String)>,
    /// (fully qualified name, span of the whole definition) of every
    /// `class`/`module` body in those files, in file order.
    pub bodies: Vec<(String, Span)>,
}

impl Selection {
    /// The ingested sources for `files` (paths relative to the app root,
    /// [`Context::root`]). A path ingest did not read is an error: a
    /// misspelt `--file` must not quietly build the identity.
    ///
    /// The bodies are named from what ingest read, unless a compiled file
    /// names a namespace by a relative path that the compiled files do not
    /// define (`class B::C` inside `module A`, where `A::B` is defined
    /// elsewhere). Ruby looks that namespace up lexically first, so the
    /// name depends on files a scoped ingest may not have read; then the
    /// compiled files are named against the app's sources, read through
    /// `context`, so every body gets the name Ruby gives it.
    pub fn resolve(app: &App, context: &Context<'_>, files: &[String]) -> Result<Selection, String> {
        let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut selected: Vec<(FileId, String)> = Vec::new();
        for file in files {
            let wanted = canonical(&context.root.join(file));
            let found = app.sources.iter().position(|source| canonical(Path::new(&source.path)) == wanted);
            let Some(index) = found else {
                return Err(format!("--file {file}: not a Ruby source the build read"));
            };
            let id = FileId(index as u32 + 1);
            if !selected.iter().any(|(selected_id, _)| *selected_id == id) {
                selected.push((id, file.clone()));
            }
        }
        let compiled: Vec<SourceFile> =
            selected.iter().filter_map(|(id, _)| (id.0 as usize).checked_sub(1).and_then(|index| app.sources.get(index))).cloned().collect();
        let mut bodies: Vec<(String, Span)> = if has_ambiguous_namespace(&compiled) {
            let compiled_paths: Vec<std::path::PathBuf> = compiled.iter().map(|source| canonical(Path::new(&source.path))).collect();
            let mut with_app = compiled.clone();
            with_app.extend(context.app_sources().iter().filter(|source| !compiled_paths.contains(&canonical(Path::new(&source.path)))).cloned());
            // The compiled files come first, so their positions map back to
            // the ids `selected` gives them.
            ConstResolver::from_app_sources(&with_app)
                .class_bodies()
                .into_iter()
                .filter_map(|(name, span)| {
                    let (id, _) = selected.get((span.file.0 as usize).checked_sub(1)?)?;
                    Some((name.to_string(), Span { file: *id, ..span }))
                })
                .collect()
        } else {
            app.const_resolver
                .for_sources(&app.sources)
                .class_bodies()
                .into_iter()
                .filter(|(_, span)| selected.iter().any(|(id, _)| *id == span.file))
                .map(|(name, span)| (name.to_string(), span))
                .collect()
        };
        bodies.sort_by_key(|(_, span)| (span.file.0, span.start));
        bodies.dedup();
        Ok(Selection { files: selected, bodies })
    }

    /// The class or module whose body is the innermost one around `at`,
    /// or `None` at a file's top level.
    pub fn owner_of(&self, at: Span) -> Option<&str> {
        if at.is_synthetic() {
            return None;
        }
        self.bodies
            .iter()
            .filter(|(_, body_span)| contains(body_span, at))
            .min_by_key(|(_, body_span)| body_span.end - body_span.start)
            .map(|(name, _)| name.as_str())
    }

    /// The bodies written in `file`.
    pub fn bodies_in(&self, file: FileId) -> Vec<(String, Span)> {
        self.bodies.iter().filter(|(_, span)| span.file == file).cloned().collect()
    }
}

pub(super) fn contains(outer: &Span, inner: Span) -> bool {
    outer.file == inner.file && outer.start <= inner.start && inner.end <= outer.end
}
