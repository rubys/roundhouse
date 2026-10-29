//! The contract every Fractal rewrite is written against.
//!
//! A pass reads one [`Unit`] (a file the build compiles) and proposes
//! edits to its text. The driver, not the pass, applies them, so every
//! pass gets the same guarantees:
//!
//! - an edit lands only inside the file it was proposed for;
//! - an edit keeps every line where it was: its replacement has exactly
//!   as many line breaks as the text it replaces, so backtraces,
//!   `__LINE__` and `source_location` in the compiled tree point at the
//!   same line as in the app's source;
//! - a proposal whose edits fall outside, overlap another edit, move a
//!   line, or leave the file with a parse error or warning it did not
//!   have is dropped whole and reported with the pass's name.
//!
//! What a pass must still guarantee itself: that the rewritten code does
//! what the original does, observably, for every caller, in the app as
//! it will run. There is no runtime fallback: the compiled tree *is* the
//! program. A pass that cannot show equivalence from what it knows at
//! build time must decline.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use crate::det::HashMap;
use std::path::{Path, PathBuf};

use crate::App;
use crate::analyze::{DefinitionIndex, definition_index};
use crate::span::{FileId, SourceFile, Span};

use super::region::Region;
use super::scope::Scope;
use super::select::Selection;

/// One compile-time rewrite.
pub trait Pass {
    /// Stable, short, kebab-case: it appears in reports.
    fn name(&self) -> &'static str;

    /// One line on what the pass does, for the pass explorer
    /// ([`super::stages`]).
    fn summary(&self) -> &'static str {
        ""
    }

    /// What the build must ingest for this pass to know what it needs.
    /// Anything outside the scope is unknown to the analyzer, so a pass
    /// that under-declares gets fewer facts and should decline where it
    /// lacks them. Facts a pass reads straight from source through
    /// [`Context::app_sources`] need no ingest.
    fn scope(&self) -> Scope;

    /// Whether the code this pass writes compiles to different bytecode
    /// from the code it replaces. `inline-delegate` writes the source Active
    /// Support would `module_eval`, so the bytecode is the same; a pass that
    /// lowers an expression to a cheaper one changes it by design, and
    /// `fractal verify` then checks that the bytecode changed only on the
    /// rewritten lines.
    fn changes_bytecode(&self) -> bool {
        false
    }

    /// Propose edits to `unit`, or decline with reasons.
    fn run(&self, context: &Context<'_>, unit: &Unit<'_>) -> Proposal;
}

/// What a pass can read besides its unit: the analyzed app, and app-wide
/// facts read from source. Each is computed the first time a pass asks,
/// and shared by every pass and unit of a build, so a build whose passes
/// ask for none reads only what it ingested.
pub struct Context<'build> {
    pub app: &'build App,
    /// The app root, canonical.
    pub root: &'build Path,
    sources: OnceCell<Vec<SourceFile>>,
    index: OnceCell<DefinitionIndex>,
    defined_methods: OnceCell<HashMap<String, Vec<String>>>,
    gem_versions: OnceCell<BTreeMap<String, String>>,
}

impl<'build> Context<'build> {
    pub fn new(app: &'build App, root: &'build Path) -> Context<'build> {
        Context { app, root, sources: OnceCell::new(), index: OnceCell::new(), defined_methods: OnceCell::new(), gem_versions: OnceCell::new() }
    }

    /// A context over sources already in memory rather than an app on
    /// disk (the browser explorer has no filesystem): `sources` are the
    /// app's Ruby files with paths relative to `root`, filtered the way
    /// [`Context::app_sources`] filters a tree, and `lockfile` is the text
    /// of `Gemfile.lock`, if the app has one.
    pub fn in_memory(app: &'build App, root: &'build Path, sources: Vec<SourceFile>, lockfile: Option<&str>) -> Context<'build> {
        let context = Context::new(app, root);
        let program = sources.into_iter().filter(|source| {
            let path = Path::new(&source.path);
            let relative = path.strip_prefix(root).unwrap_or(path);
            relative.extension().is_some_and(|extension| extension == "rb")
                && !relative.ancestors().skip(1).any(|directory| super::scope::skip_program_dir(Path::new(""), directory, NOT_PROGRAM))
        });
        let _ = context.sources.set(program.collect());
        let _ = context.gem_versions.set(lockfile.map(parse_lockfile).unwrap_or_default());
        context
    }

    /// Every Ruby file of the app the program can load, read from disk
    /// (not through ingest's scope), with absolute paths. Tests, specs
    /// and generated directories are left out: they are not the program
    /// the compiled tree ships.
    pub fn app_sources(&self) -> &[SourceFile] {
        self.sources.get_or_init(|| app_ruby_sources(self.root))
    }

    /// A Rubydex index of [`Context::app_sources`]: where each class and
    /// module is defined, and its ancestors.
    pub fn index(&self) -> &DefinitionIndex {
        self.index.get_or_init(|| definition_index(self.app_sources()))
    }

    /// The app's classes and modules that have `name` among their
    /// ancestors.
    pub fn descendants(&self, name: &str) -> Vec<&str> {
        let mut descendants: Vec<&str> = self
            .index()
            .ancestors
            .iter()
            .filter(|(_, ancestors)| ancestors.iter().any(|ancestor| ancestor.as_ref() == name))
            .map(|(descendant, _)| descendant.as_ref())
            .collect();
        descendants.sort();
        descendants
    }

    /// Method names the app's own source defines anywhere, by `def`,
    /// `define_method`, `alias`, `alias_method`, `attr_*` or `delegate`,
    /// each with the `path:line` sites that define it. A pass that relies
    /// on a framework method keeping its framework definition checks its
    /// name here: an app definition of that name anywhere may be the one
    /// that runs. Computed names are recorded under
    /// [`super::definitions::DYNAMIC`].
    pub fn defined_methods(&self) -> &HashMap<String, Vec<String>> {
        self.defined_methods.get_or_init(|| {
            let mut definitions: HashMap<String, Vec<String>> = HashMap::default();
            for source in self.app_sources() {
                super::definitions::collect(source, self.root, &mut definitions);
            }
            definitions
        })
    }

    /// The version `Gemfile.lock` resolves `gem` to.
    pub fn gem_version(&self, gem: &str) -> Option<&str> {
        self.gem_versions.get_or_init(|| lockfile_versions(&self.root.join("Gemfile.lock"))).get(gem).map(String::as_str)
    }

    /// `path` relative to the app root, for messages.
    pub fn relative(&self, path: &str) -> String {
        Path::new(path).strip_prefix(self.root).map(|relative| relative.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.to_string())
    }
}

/// One file the build compiles, as a pass sees it.
pub struct Unit<'build> {
    pub file: FileId,
    /// Relative to the app root.
    pub path: String,
    pub text: &'build str,
    /// The `class`/`module` bodies in this file: (fully qualified name,
    /// span of the whole definition).
    pub bodies: Vec<(String, Span)>,
    /// The methods written in this file, with the analyzer's facts.
    pub methods: Vec<Region<'build>>,
    pub(super) selection: &'build Selection,
}

impl Unit<'_> {
    /// The class or module whose body is the innermost one around `at`,
    /// or `None` at the file's top level.
    pub fn owner_of(&self, at: Span) -> Option<&str> {
        self.selection.owner_of(at)
    }

    /// The class or module a `class`/`module` definition at exactly
    /// `span` is a body of.
    pub fn body_at(&self, span: Span) -> Option<&str> {
        self.bodies.iter().find(|(_, body_span)| *body_span == span).map(|(name, _)| name.as_str())
    }

    /// A span of this unit's file.
    pub fn span(&self, start: usize, end: usize) -> Span {
        Span { file: self.file, start: start as u32, end: end as u32 }
    }

    /// The file's `frozen_string_literal` magic comment says true.
    pub fn frozen_string_literals(&self) -> bool {
        let parsed = ruby_prism::parse(self.text.as_bytes());
        parsed.magic_comments().any(|comment| {
            let key = String::from_utf8_lossy(comment.key()).to_ascii_lowercase().replace('-', "_");
            key == "frozen_string_literal" && String::from_utf8_lossy(comment.value()).trim().eq_ignore_ascii_case("true")
        })
    }
}

/// A pass's answer for one unit.
#[derive(Clone, Default)]
pub struct Proposal {
    pub edits: Vec<Edit>,
    /// Sites the pass looked at and left alone, with why.
    pub declined: Vec<(Span, String)>,
}

/// Replace the source text at `span` with `text`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub span: Span,
    pub text: String,
}

/// The passes a build runs, in order. See `docs/pipeline/fractal.md`.
/// Call it once per build: a pass may keep what it learned about the app
/// (an app-wide read) for the rest of the build.
pub fn registered() -> Vec<Box<dyn Pass>> {
    vec![Box::new(super::passes::InlineDelegate::default()), Box::new(super::passes::LowerDuration::default())]
}

/// Directories [`Context::app_sources`] skips only at the app root.
const NOT_PROGRAM: &[&str] = &["tmp", "log", "coverage", "public", "test", "spec", "sorbet", "sig"];

fn app_ruby_sources(root: &Path) -> Vec<SourceFile> {
    let mut paths: Vec<PathBuf> = Vec::new();
    super::scope::walk(root, &|directory| super::scope::skip_source_dir(root, directory, NOT_PROGRAM), &mut |path| {
        if path.extension().is_some_and(|extension| extension == "rb") {
            paths.push(path.to_path_buf());
        }
    });
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let bytes = std::fs::read(&path).ok()?;
            Some(SourceFile { path: path.to_string_lossy().into_owned(), text: String::from_utf8_lossy(&bytes).into_owned() })
        })
        .collect()
}

/// `gem => version` from a lockfile's specs (the four-space-indented
/// `name (version)` lines).
fn lockfile_versions(path: &Path) -> BTreeMap<String, String> {
    std::fs::read_to_string(path).map(|text| parse_lockfile(&text)).unwrap_or_default()
}

fn parse_lockfile(text: &str) -> BTreeMap<String, String> {
    let mut versions = BTreeMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("    ") else { continue };
        if rest.starts_with(' ') {
            continue;
        }
        let Some((name, version)) = rest.split_once(" (") else { continue };
        let Some(version) = version.strip_suffix(')') else { continue };
        // Platform-specific entries carry the platform after the version.
        let version = version.split('-').next().unwrap_or(version);
        versions.entry(name.to_string()).or_insert_with(|| version.to_string());
    }
    versions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lockfile_versions_reads_specs_and_strips_platforms() {
        let directory = std::env::temp_dir().join(format!("fractal-lock-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let lock = directory.join("Gemfile.lock");
        std::fs::write(
            &lock,
            "GEM\n  remote: https://rubygems.org/\n  specs:\n    activerecord (8.1.4)\n      activemodel (= 8.1.4)\n    sqlite3 (2.10.1-x86_64-linux-gnu)\n\nBUNDLED WITH\n   2.6.9\n",
        )
        .unwrap();
        let versions = lockfile_versions(&lock);
        assert_eq!(versions.get("activerecord").map(String::as_str), Some("8.1.4"));
        assert_eq!(versions.get("sqlite3").map(String::as_str), Some("2.10.1"));
        assert_eq!(versions.get("activemodel"), None);
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
