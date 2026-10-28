//! `--target fractal`: compile a Rails app by rewriting its own source,
//! one file at a time.
//!
//! A person names the files to compile (`--file app/models/article.rb`).
//! Roundhouse ingests and analyzes those files and what its passes say
//! they need ([`scope`]), so it knows what Rails cannot see from one call
//! site (types, effects, which definitions exist), and passes use that
//! application knowledge to rewrite those files. The output holds only
//! changed files at app-relative paths ([`tree`]). The separate [`apply`]
//! step overwrites those files in the base app. Nothing installs code at
//! boot or checks assumptions at load: the rewritten code is the only
//! code there is.
//!
//! The pieces, in the order a build runs them:
//!
//! - [`scope`]: which files to ingest besides the compiled ones, decided
//!   from a Rubydex index of the app's constants before anything is
//!   ingested.
//! - [`select`]: the compiled files, and every `class`/`module` body in
//!   them (so a pass knows which class a site belongs to).
//! - [`region`]: each `def` in a compiled file, with the analyzer's facts
//!   about it.
//! - [`pass`]: the rewrites. Each reads one compiled file (a [`Unit`])
//!   and proposes edits anywhere in it. [`passes`] holds the registered
//!   ones.
//! - [`rewrite`]: applies the proposals, refusing any that leave the
//!   file, overlap, move a line, or break the parse.
//! - [`tree`]: emits the sparse overlay and applies it to the base app.
//! - [`verify`]: boots the app before and after the overlay and compares
//!   what Ruby can observe of the compiled code (not in the browser build).
//!
//! [`stages`] is not part of a build: it shows the compiled files after
//! each pass, for the browser pass explorer (`wasm/fractal/`).
//!
//! Every assumption a rewrite rests on is checked at build time; there is
//! no runtime fallback. A rewrite that cannot be shown equivalent from
//! what the build knows is declined and reported. See
//! `docs/pipeline/fractal.md`.

mod definitions;
pub mod pass;
pub mod passes;
pub mod region;
pub mod rewrite;
pub mod scope;
pub mod select;
pub mod stages;
pub mod tree;
#[cfg(not(target_arch = "wasm32"))]
pub mod verify;

use std::path::Path;

use serde::Serialize;

use crate::App;

pub use pass::{Context, Edit, Pass, Proposal, Unit};
pub use region::{Analyzed, Knowledge, Region};
pub use rewrite::{Decline, Rewrite};
pub use scope::{Ingest, Reach, Scope, ScopeContext};
pub use select::Selection;
pub use tree::{MANIFEST, apply};

/// What a build decided, for the CLI's report and `--report PATH`.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    /// The files the build compiles, relative to the app root.
    pub compiled: Vec<String>,
    pub passes: Vec<String>,
    pub methods: Vec<MethodReport>,
    /// Compiled files no pass may rewrite, with why.
    pub skipped: Vec<Skipped>,
    /// Every edit applied, in file order.
    pub rewrites: Vec<Rewrite>,
    /// The rewritten files in the overlay, relative to the app root.
    pub rewritten: Vec<String>,
    pub declined: Vec<Decline>,
}

#[derive(Debug, Serialize)]
pub struct MethodReport {
    pub key: String,
    pub knowledge: Knowledge,
}

#[derive(Debug, Serialize)]
pub struct Skipped {
    pub file: String,
    pub reason: String,
}

impl Report {
    /// The human-readable summary the CLI prints.
    pub fn render(&self) -> String {
        let mut summary = String::new();
        if self.compiled.is_empty() {
            summary.push_str("fractal: no --file given, so nothing to compile\n");
            return summary;
        }
        for file in &self.compiled {
            summary.push_str(&format!("fractal: compiling {file}\n"));
        }
        let typed = self.methods.iter().filter(|method| method.knowledge.copies > 0 && method.knowledge.typed == method.knowledge.expressions).count();
        summary.push_str(&format!(
            "fractal: {} method(s) in the compiled files ({typed} fully typed); {} pass(es): {}\n",
            self.methods.len(),
            self.passes.len(),
            if self.passes.is_empty() { "none registered".to_string() } else { self.passes.join(", ") },
        ));
        for skipped in &self.skipped {
            summary.push_str(&format!("  skipped       {}: {}\n", skipped.file, skipped.reason));
        }
        summary.push_str(&format!("fractal: {} rewrite(s) in {} file(s)\n", self.rewrites.len(), self.rewritten.len()));
        // One line per rewrite: a multi-line edit's line breaks print as `\n`.
        let one_line = |text: &str| text.replace('\n', "\\n");
        for rewrite in &self.rewrites {
            summary.push_str(&format!("  rewrote       {} [{}]: {} -> {}\n", rewrite.at, rewrite.pass, one_line(&rewrite.from), one_line(&rewrite.to)));
        }
        for decline in &self.declined {
            summary.push_str(&format!("  declined      {} [{}]: {}\n", decline.at, decline.pass, decline.reason));
        }
        summary
    }
}

/// A build's decisions and the files it rewrites.
pub struct Plan {
    pub report: Report,
    /// (path relative to the app root, rewritten text).
    pub rewritten: Vec<(String, String)>,
}

impl Plan {
    /// The [`MANIFEST`] the emitted overlay carries.
    pub fn manifest(&self) -> String {
        #[derive(Serialize)]
        struct Manifest<'report> {
            compiled: &'report [String],
            passes: &'report [String],
            rewritten: &'report [String],
        }
        let manifest = Manifest { compiled: &self.report.compiled, passes: &self.report.passes, rewritten: &self.report.rewritten };
        serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n"
    }
}

/// Decide a build: run `passes` over each of `files` (paths relative to
/// the app root `input`) and apply what they propose. `untrusted` names
/// files ingest could only partly read (a `--survey` gap or a syntax
/// error), which keep the app's code.
pub fn plan(app: &App, input: &Path, files: &[String], untrusted: &[String], passes: &[&dyn Pass]) -> Result<Plan, String> {
    let mut report = Report {
        compiled: files.to_vec(),
        passes: passes.iter().map(|pass| pass.name().to_string()).collect(),
        ..Report::default()
    };
    if files.is_empty() {
        return Ok(Plan { report, rewritten: Vec::new() });
    }
    let root = input.canonicalize().map_err(|error| format!("input {}: {error}", input.display()))?;
    let context = Context::new(app, &root);
    let selection = Selection::resolve(app, &context, files)?;
    report.compiled = selection.files.iter().map(|(_, path)| path.clone()).collect();
    let is_untrusted = |path: &str| untrusted.iter().any(|untrusted_path| is_suffix_path(path, untrusted_path) || is_suffix_path(untrusted_path, path));
    let mut rewritten = Vec::new();
    for (file, path) in &selection.files {
        let Some(source) = (file.0 as usize).checked_sub(1).and_then(|index| app.sources.get(index)) else { continue };
        let methods = region::find(app, &selection, *file, path);
        report.methods.extend(methods.iter().map(|method| MethodReport { key: method.key(), knowledge: method.knowledge.clone() }));
        let skip = if is_untrusted(&source.path) {
            Some("ingest could not read the file faithfully (a --survey gap or a syntax error)".to_string())
        } else if std::fs::read(root.join(path)).ok().as_deref() != Some(source.text.as_bytes()) {
            Some("the file on disk is not the UTF-8 text ingest read".to_string())
        } else {
            None
        };
        if let Some(reason) = skip {
            report.skipped.push(Skipped { file: path.clone(), reason });
            continue;
        }
        if passes.is_empty() {
            continue;
        }
        let unit = Unit { file: *file, path: path.clone(), text: &source.text, bodies: selection.bodies_in(*file), methods, selection: &selection };
        let proposals = passes.iter().map(|pass| (pass.name(), pass.run(&context, &unit))).collect();
        let outcome = rewrite::apply(&unit, proposals);
        report.rewrites.extend(outcome.rewrites);
        report.declined.extend(outcome.declined);
        if let Some(text) = outcome.text.filter(|text| text != &source.text) {
            report.rewritten.push(path.clone());
            rewritten.push((path.clone(), text));
        }
    }
    report.methods.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(Plan { report, rewritten })
}

/// Write only changed files and the build manifest to `overlay`. The base
/// app stays unchanged until [`apply`] copies the overlay onto it.
pub fn write(input: &Path, overlay: &Path, plan: &Plan) -> Result<tree::Written, String> {
    tree::write(input, overlay, &plan.rewritten, &plan.manifest())
}

/// Is `relative_path` the tail of `path` at a path boundary?
fn is_suffix_path(path: &str, relative_path: &str) -> bool {
    path == relative_path || path.ends_with(&format!("/{relative_path}"))
}
