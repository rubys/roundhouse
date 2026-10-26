//! What a fractal build ingests.
//!
//! A build compiles the files named with `--file`, and by default it
//! ingests only those files, plus what each registered pass says it needs
//! to know about them ([`Pass::scope`]). Rubydex answers where those other
//! files are before anything is ingested: it indexes the app's Ruby for
//! constants alone, which is far cheaper than ingesting and analyzing
//! it. A pass whose knowledge can come from anywhere in the app says so
//! with [`Scope::WholeApp`], and `--whole-app` asks for everything
//! whatever the passes need.
//!
//! Outside the scope, files read as absent. What they would have told
//! the analyzer (a column's type, a method another file defines) is then
//! unknown, and a pass that needed it declines: a narrower scope can
//! produce fewer rewrites, never a different one.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::analyze::{DefinitionIndex, definition_index};
use crate::span::SourceFile;

use super::pass::Pass;

/// What a pass needs ingested.
pub enum Scope {
    /// Application knowledge from anywhere: ingest and analyze the whole
    /// app, as a build with `--whole-app` does.
    WholeApp,
    /// The files the build compiles, plus what each [`Reach`] adds.
    Files(Vec<Reach>),
}

/// One kind of file a [`Scope::Files`] pass also needs.
#[derive(Clone, Copy)]
pub enum Reach {
    /// The files defining the ancestors, that the app defines itself, of
    /// each class or module the compiled files define: superclasses and included, prepended and extended
    /// modules, transitively. What a method inherits, and what an
    /// inherited `attr_reader`'s signature says, comes from these.
    Ancestors,
    /// Every `sig/**/*.rbs` sidecar.
    Signatures,
    /// The schema (`db/schema.rb`, else `db/structure.sql`), for column
    /// types.
    Schema,
    /// Anything else: the function adds paths (absolute, under
    /// [`ScopeCx::root`]) from the Rubydex index.
    Custom(fn(&ScopeCx<'_>, &mut BTreeSet<PathBuf>)),
}

/// What a [`Reach::Custom`] function can read.
pub struct ScopeCx<'a> {
    /// The app root, canonical.
    pub root: &'a Path,
    /// The files the build compiles, relative to `root`.
    pub files: &'a [String],
    /// The classes and modules those files define, fully qualified.
    pub classes: &'a [String],
    pub index: &'a DefinitionIndex,
}

/// The build's decision.
pub enum Ingest {
    /// Ingest the whole app, and why.
    WholeApp { reason: String },
    /// Ingest these files (absolute) and nothing else. `indexed` is how
    /// many Ruby files Rubydex read to decide.
    Files { files: BTreeSet<PathBuf>, indexed: usize },
}

/// Files ingest reads to learn the app's shape (its gems, inflections,
/// packages and autoload settings), kept in every scope.
const APP_SHAPE: &[&str] =
    &["Gemfile.lock", "roundhouse.yml", "packwerk.yml", "packs.yml", "config/application.rb", "config/initializers/inflections.rb"];

/// Directories excluded only at the app root. Nested directories can
/// define application namespaces with these names.
const UNINDEXED: &[&str] = &["vendor", "tmp", "log", "coverage", "public", "test", "spec", "db", "sorbet", "sig"];

/// `--file` arguments as paths relative to the app root `root`
/// (canonical): relative arguments are taken from the root, absolute ones
/// must lie under it, and each must be a file.
pub fn normalize_files(root: &Path, files: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for file in files {
        let path = root.join(file);
        let Ok(canonical) = path.canonicalize() else {
            return Err(format!("--file {file}: no such file in the app"));
        };
        let Ok(rel) = canonical.strip_prefix(root) else {
            return Err(format!("--file {file}: outside the app"));
        };
        if !canonical.is_file() {
            return Err(format!("--file {file}: not a file"));
        }
        let rel = rel.to_string_lossy().replace('\\', "/");
        if !out.contains(&rel) {
            out.push(rel);
        }
    }
    Ok(out)
}

/// Decide what a build compiling `files` (relative to `root`, canonical)
/// with `passes` ingests. `whole_app` is `--whole-app`.
pub fn decide(root: &Path, files: &[String], passes: &[&dyn Pass], whole_app: bool) -> Result<Ingest, String> {
    if whole_app {
        return Ok(Ingest::WholeApp { reason: "--whole-app".to_string() });
    }
    let mut reaches = Vec::new();
    for pass in passes {
        match pass.scope() {
            Scope::WholeApp => return Ok(Ingest::WholeApp { reason: format!("pass `{}` needs the whole app", pass.name()) }),
            Scope::Files(r) => reaches.extend(r),
        }
    }
    let mut kept = BTreeSet::new();
    let mut indexed = 0;
    if !files.is_empty() {
        for file in files {
            let path = root.join(file);
            if !path.is_file() {
                return Err(format!("--file {file}: no such file in the app"));
            }
            kept.insert(path);
        }
        if !reaches.is_empty() {
            let sources = ruby_sources(root);
            indexed = sources.len();
            let index = definition_index(&sources);
            drop(sources);
            let compiled: Vec<String> = kept.iter().map(|p| p.to_string_lossy().into_owned()).collect();
            let mut classes: Vec<String> = index
                .files
                .iter()
                .filter(|(_, paths)| paths.iter().any(|p| compiled.contains(p)))
                .map(|(name, _)| name.to_string())
                .collect();
            classes.sort();
            let cx = ScopeCx { root, files, classes: &classes, index: &index };
            for reach in reaches {
                match reach {
                    Reach::Ancestors => {
                        for name in &classes {
                            for ancestor in index.ancestors.get(name.as_str()).into_iter().flatten() {
                                kept.extend(index.files.get(ancestor).into_iter().flatten().map(PathBuf::from));
                            }
                        }
                    }
                    Reach::Signatures => walk(&root.join("sig"), &|_| false, &mut |path| {
                        if path.extension().is_some_and(|e| e == "rbs") {
                            kept.insert(path.to_path_buf());
                        }
                    }),
                    Reach::Schema => {
                        let schema = ["db/schema.rb", "db/structure.sql"].iter().map(|p| root.join(p)).find(|p| p.is_file());
                        kept.extend(schema);
                    }
                    Reach::Custom(widen) => widen(&cx, &mut kept),
                }
            }
        }
    }
    kept.extend(layout_files(root, &kept));
    kept.extend(APP_SHAPE.iter().map(|p| root.join(p)).filter(|p| p.is_file()));
    Ok(Ingest::Files { files: kept, indexed })
}

/// What tells ingest that a directory holding a kept file is an app root:
/// a Packwerk `package.yml`, or the `Rails::Engine` subclass in the `lib/`
/// of an in-repository gem (a component with its own `app/`).
fn layout_files(root: &Path, files: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let mut dirs = BTreeSet::new();
    for file in files {
        dirs.extend(file.ancestors().skip(1).take_while(|dir| dir.starts_with(root) && *dir != root).map(Path::to_path_buf));
    }
    let mut out = BTreeSet::new();
    for dir in dirs {
        let package = dir.join("package.yml");
        if package.is_file() {
            out.insert(package);
        }
        let is_gem = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry.path().extension().is_some_and(|e| e == "gemspec"));
        if is_gem && dir.join("app").is_dir() {
            walk(&dir.join("lib"), &|_| false, &mut |path| {
                let declares = path.extension().is_some_and(|e| e == "rb")
                    && std::fs::read_to_string(path).is_ok_and(|text| text.contains("Rails::Engine"));
                if declares {
                    out.insert(path.to_path_buf());
                }
            });
        }
    }
    out
}

/// Exclude root-level non-program directories, dependency trees at any
/// depth, and earlier emitted overlays.
pub(super) fn skip_source_dir(root: &Path, dir: &Path, root_only: &[&str]) -> bool {
    skip_program_dir(root, dir, root_only) || dir.join(super::tree::MANIFEST).is_file()
}

/// Directory exclusions shared by disk and in-memory source trees.
pub(super) fn skip_program_dir(root: &Path, dir: &Path, root_only: &[&str]) -> bool {
    let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("");
    name.starts_with('.') || name == "node_modules" || (dir.parent() == Some(root) && root_only.contains(&name))
}

/// Every `.rb` under `root` outside [`UNINDEXED`] and hidden directories,
/// with absolute paths, for the Rubydex index.
fn ruby_sources(root: &Path) -> Vec<SourceFile> {
    let mut paths = Vec::new();
    walk(root, &|dir| skip_source_dir(root, dir, UNINDEXED), &mut |p| {
        if p.extension().is_some_and(|e| e == "rb") {
            paths.push(p.to_path_buf());
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

/// Visit every file under `dir`, not following symlinked directories and
/// skipping hidden directories and the ones `skip` names.
pub(super) fn walk(dir: &Path, skip: &dyn Fn(&Path) -> bool, visit: &mut dyn FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with('.') && !skip(&path) {
                walk(&path, skip, visit);
            }
        } else if kind.is_file() {
            visit(&path);
        }
    }
}
