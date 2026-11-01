//! Opt-in, check-only declarations from locally installed, locked gems.
//!
//! This is a producer for the existing RBI gem boundary and Rubydex's
//! declaration index, not a source-gem mount. No gem or application code
//! is executed. Method bodies, initializers and value expressions never
//! enter the app IR. The resulting surface is deliberately incomplete:
//! absent method signatures remain gradual, rather than asserting a
//! closed API. Loading and target availability still need their own facts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::gem_boundary::GemBoundary;
use crate::span::SourceFile;
use crate::vfs::Vfs;

use super::source_declarations::{
    OPAQUE_CONSTANTS, constant_effect, read_rbs as rbs_declarations,
    read_ruby_for_check as ruby_declarations,
};

/// Counts for the explicit check input.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub gems_found: usize,
    pub gems_missing: usize,
    pub ruby_files: usize,
    pub rbs_files: usize,
    pub rejected_files: usize,
    /// Whole-file rejection reasons from the reader, with occurrence counts.
    /// Keep these visible rather than treating rejected inputs as empty files.
    pub rejection_reasons: BTreeMap<String, usize>,
    pub declaration_bytes: usize,
    pub non_ruby_specs_rejected: usize,
    pub native_gems_incomplete: usize,
    pub git_specs_rejected: usize,
    pub collection_contracts_skipped: usize,
    pub collection_version_conflicts: usize,
    pub trusted_rbs_inputs: usize,
}

#[derive(Default)]
pub struct Inputs {
    pub gem_paths: Vec<PathBuf>,
    pub rbs_paths: Vec<PathBuf>,
    pub(crate) report: std::cell::RefCell<Report>,
}

impl Inputs {
    pub fn report(&self) -> Report {
        self.report.borrow().clone()
    }
}

/// Retained for focused source-mode callers. The check CLI prepares these
/// inputs before the app's single Rubydex pass, so written names agree with
/// the registry built during ingest.
#[cfg(test)]
pub(crate) fn load<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    app: &mut App,
    gem_paths: &[PathBuf],
    rbs_paths: &[PathBuf],
) -> Report {
    let inputs = Inputs {
        gem_paths: gem_paths.to_vec(),
        rbs_paths: rbs_paths.to_vec(),
        ..Inputs::default()
    };
    let declarations = prepare(vfs, dir, app, &inputs);
    if !declarations.is_empty() {
        app.const_resolver = crate::analyze::PreparedConstResolver::with_external_declarations(
            &app.sources,
            declarations,
        );
    }
    inputs.report()
}

struct Extracted {
    declarations: Vec<SourceFile>,
    gems: BTreeMap<String, String>,
    report: Report,
}

pub(crate) fn prepare<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    app: &mut App,
    inputs: &Inputs,
) -> Vec<SourceFile> {
    let lock = app.gem_lock.clone().unwrap_or_default();
    let extracted = extract(vfs, dir, &lock, inputs);
    finish(vfs, dir, app, inputs, extracted)
}

fn extract<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    lock: &crate::gems::Lockfile,
    inputs: &Inputs,
) -> Extracted {
    let mut report = Report::default();
    let mut roots = inputs.gem_paths.clone();
    roots.extend(crate::i18n::gems::gem_roots(vfs, dir));
    let mut tasks = Vec::new();
    let mut declarations = Vec::new();
    let mut gems = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (gem, version) in &lock.specs {
        if lock.is_in_repo(gem) || lock.is_source(gem) {
            continue;
        }
        // GEM releases on the ruby platform only. Neither a nearby version,
        // a native build nor a Git checkout attests the requested release.
        if lock.is_from_git(gem) {
            report.git_specs_rejected += 1;
            continue;
        }
        let (locked_release, platform) = spec_identity(version, &lock.platforms);
        if platform != "ruby" {
            report.non_ruby_specs_rejected += 1;
            continue;
        }
        let installed = roots
            .iter()
            .map(|root| root.join("gems").join(format!("{gem}-{locked_release}")))
            .find(|path| vfs.is_dir(path));
        let Some(installed) = installed else {
            report.gems_missing += 1;
            continue;
        };
        report.gems_found += 1;
        let canonical = vfs.canonical(&installed).unwrap_or(installed);
        if !seen.insert(canonical.clone()) {
            continue;
        }
        let native = vfs.is_dir(&canonical.join("ext"));
        report.native_gems_incomplete += usize::from(native);
        let mut read_errors = BTreeMap::new();
        let mut paths = files(vfs, &canonical.join("lib"), "rb", &mut read_errors);
        paths.extend(files(vfs, &canonical.join("app"), "rb", &mut read_errors));
        paths.sort();
        tasks.push(ExtractTask {
            root: canonical,
            paths,
            gem: gem.clone(),
            document_prefix: format!("{gem}-{locked_release}"),
            rbs: false,
            native,
            read_errors,
        });
    }
    report.trusted_rbs_inputs = inputs.rbs_paths.len();
    for (ordinal, root) in inputs.rbs_paths.iter().enumerate() {
        let root = vfs.canonical(root).unwrap_or_else(|| root.clone());
        let mut read_errors = BTreeMap::new();
        let paths = if vfs.is_dir(&root) {
            files(vfs, &root, "rbs", &mut read_errors)
        } else {
            vec![root.clone()]
        };
        let base = if vfs.is_dir(&root) {
            root.clone()
        } else {
            root.parent().unwrap_or(Path::new("")).to_path_buf()
        };
        tasks.push(ExtractTask {
            root: base,
            paths,
            gem: "rbs-input".into(),
            document_prefix: format!("rbs-input/{ordinal}"),
            rbs: true,
            native: false,
            read_errors,
        });
    }
    if let Ok(text) = vfs.read_to_string(&dir.join("rbs_collection.lock.yaml")) {
        if let Ok(collection) = serde_yaml_ng::from_str::<Collection>(&text) {
            for gem in collection.gems {
                if !lock.has(&gem.name) || lock.is_in_repo(&gem.name) || lock.is_source(&gem.name) {
                    continue;
                }
                // A selected community version is not evidence that its
                // declarations exist in the locked Ruby release. Automatic
                // collection admission is disabled. A caller may explicitly
                // pass a signature path as a trusted contract instead.
                report.collection_contracts_skipped += 1;
                let locked = lock.version_of(&gem.name).unwrap_or("");
                if spec_identity(locked, &lock.platforms).0 != gem.version {
                    report.collection_version_conflicts += 1;
                }
            }
        }
    }
    for task in tasks {
        let payload = extract_sources(vfs, &task);
        merge_payload(payload, &mut declarations, &mut gems, &mut report);
    }
    Extracted {
        declarations,
        gems,
        report,
    }
}

fn finish<V: Vfs + ?Sized>(
    vfs: &V,
    dir: &Path,
    app: &mut App,
    inputs: &Inputs,
    extracted: Extracted,
) -> Vec<SourceFile> {
    let Extracted {
        mut declarations,
        gems,
        mut report,
    } = extracted;
    if declarations.is_empty() {
        *inputs.report.borrow_mut() = report;
        return Vec::new();
    }
    let sources = if app.sources.is_empty() {
        super::sources::snapshot()
    } else {
        app.sources.clone()
    };
    // Zeitwerk gives a directory with no same-name .rb file its own
    // namespace. Record those app-owned prefixes before imported ancestor
    // constants can capture a compact declaration (Category::Query::Search).
    let known: BTreeSet<_> = app
        .models
        .iter()
        .map(|c| c.name.0.as_str())
        .chain(app.controllers.iter().map(|c| c.name.0.as_str()))
        .chain(app.library_classes.iter().map(|c| c.name.0.as_str()))
        .collect();
    let roots: Vec<_> = super::app::app_roots(vfs, dir, &[])
        .iter()
        .flat_map(|root| vfs.read_dir(&dir.join(root)).unwrap_or_default())
        .filter(|root| vfs.is_dir(root))
        .flat_map(|root| {
            let concerns = root.join("concerns");
            if vfs.is_dir(&concerns) {
                vec![root, concerns]
            } else {
                vec![root]
            }
        })
        .collect();
    let mut implicit = BTreeSet::new();
    for name in &known {
        for (at, _) in name.match_indices("::") {
            let prefix = &name[..at];
            if known.contains(prefix)
                || app
                    .gem_boundary
                    .classes
                    .contains_key(&crate::ident::ClassId(crate::ident::Symbol::new(prefix)))
            {
                continue;
            }
            let relative = crate::naming::underscore(prefix);
            if roots.iter().any(|root| {
                let path = root.join(&relative);
                vfs.is_dir(&path) && !vfs.exists(&path.with_extension("rb"))
            }) {
                implicit.insert(prefix);
            }
        }
    }
    if !implicit.is_empty() {
        let declared = crate::analyze::external_definition_names(&sources, &declarations);
        implicit.retain(|name| !declared.contains(*name));
    }
    if !implicit.is_empty() {
        declarations.push(SourceFile {
            path: "app-implicit-namespaces.rb".into(),
            text: implicit
                .into_iter()
                .map(|name| format!("module ::{name}\nend\n"))
                .collect(),
        });
    }
    // An opaque root effect contributes the same negative lookup fact in
    // every file. Keep one barrier, rather than thousands of indistinguishable
    // unresolved Object mixins (which Rubydex expands separately).
    let root_barrier = format!("class ::Object\ninclude ::{OPAQUE_CONSTANTS}\nend\n");
    let mut root_incomplete = false;
    for source in &mut declarations {
        if source.text.contains(&root_barrier) {
            source.text = source.text.replace(&root_barrier, "");
            root_incomplete = true;
        }
    }
    let named_root = regex::Regex::new(
        r"class ::Object\ninclude ::(RoundhouseUncertainNameA8_[0-9a-f]+)\nend\n",
    )
    .unwrap();
    let mut root_names = BTreeSet::new();
    for source in &mut declarations {
        for capture in named_root.captures_iter(&source.text) {
            root_names.insert(capture[1].to_string());
        }
        source.text = named_root.replace_all(&source.text, "").into_owned();
    }
    if !root_incomplete && !root_names.is_empty() {
        declarations.push(SourceFile {
            path: "uncertain-root-names.rb".into(),
            text: root_names
                .into_iter()
                .map(|marker| format!("class ::Object\ninclude ::{marker}\nend\n"))
                .collect(),
        });
    }
    declarations.retain(|source| !source.text.is_empty());
    if root_incomplete {
        declarations.push(SourceFile {
            path: "incomplete-root.rb".into(),
            text: root_barrier,
        });
    }
    let sources = if app.sources.is_empty() {
        super::sources::snapshot()
    } else {
        app.sources.clone()
    };
    declarations = crate::analyze::canonical_external_declarations(&sources, &declarations);
    let mut canonical = GemBoundary::default();
    for source in &declarations {
        if let Some(gem) = gems.get(&source.path) {
            super::rbi::read_rbi(source.text.as_bytes(), gem, &mut canonical);
        }
    }
    let unbounded_effects = declarations.iter().any(|source| {
        source
            .text
            .contains(super::source_declarations::UNBOUNDED_EFFECT)
    });
    let mut classes: Vec<_> = canonical.classes.into_iter().collect();
    classes.sort_by(|a, b| a.0.0.as_str().cmp(b.0.0.as_str()));
    for (id, mut class) in classes {
        class.dynamic = true;
        class.declarations_only = true;
        class.constant_namespace_incomplete = unbounded_effects
            || class
                .includes
                .iter()
                .chain(&class.extends)
                .any(|id| constant_effect(id.0.as_str()).is_some());
        class
            .includes
            .retain(|id| constant_effect(id.0.as_str()).is_none());
        class
            .extends
            .retain(|id| constant_effect(id.0.as_str()).is_none());
        let incomplete = class.constant_namespace_incomplete;
        app.gem_boundary
            .classes
            .entry(id)
            .and_modify(|existing| existing.constant_namespace_incomplete |= incomplete)
            .or_insert(class);
    }
    report.declaration_bytes = declarations.iter().map(|source| source.text.len()).sum();
    *inputs.report.borrow_mut() = report;
    declarations
}

fn spec_identity<'a>(version: &'a str, platforms: &[String]) -> (&'a str, &'a str) {
    let mut platforms: Vec<_> = platforms.iter().filter(|p| p.as_str() != "ruby").collect();
    platforms.sort_by_key(|p| std::cmp::Reverse(p.len()));
    for platform in platforms {
        for abi in ["", "-gnu", "-musl"] {
            let suffix = format!("-{platform}{abi}");
            if let Some(release) = version.strip_suffix(&suffix) {
                return (release, &version[release.len() + 1..]);
            }
        }
    }
    static PLATFORM: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(.+?)-((?:aarch64|arm64|armv[0-9]+|universal|x86_64|x86|x64|i[3-6]86|java|jruby|ppc64le|powerpc|sparc|wasm32|riscv64|s390x)(?:[-_].*)?)$").unwrap()
    });
    if let Some(parts) = PLATFORM.captures(version) {
        return (
            parts.get(1).unwrap().as_str(),
            parts.get(2).unwrap().as_str(),
        );
    }
    (version, "ruby")
}

#[derive(Default)]
struct Payload {
    documents: Vec<SourceFile>,
    gem: String,
    ruby_files: usize,
    rbs_files: usize,
    rejection_reasons: BTreeMap<String, usize>,
}

struct ExtractTask {
    root: PathBuf,
    paths: Vec<PathBuf>,
    gem: String,
    document_prefix: String,
    rbs: bool,
    native: bool,
    read_errors: BTreeMap<String, usize>,
}

fn extract_sources<V: Vfs + ?Sized>(vfs: &V, task: &ExtractTask) -> Payload {
    let mut payload = Payload {
        gem: task.gem.clone(),
        rejection_reasons: task.read_errors.clone(),
        ..Payload::default()
    };
    for path in &task.paths {
        let bytes = match vfs.read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                *payload
                    .rejection_reasons
                    .entry(format!(
                        "could not read declaration source: {:?}",
                        error.kind()
                    ))
                    .or_default() += 1;
                continue;
            }
        };
        let relative = path
            .strip_prefix(&task.root)
            .unwrap_or(path)
            .to_string_lossy();
        let text = if task.rbs {
            std::str::from_utf8(&bytes)
                .map_err(|error| {
                    format!(
                        "non-UTF-8 RBS source at byte {} is not supported",
                        error.valid_up_to()
                    )
                })
                .and_then(rbs_declarations)
        } else {
            ruby_declarations(&bytes)
        };
        let text = match text {
            Ok(text) => text,
            Err(reason) => {
                *payload.rejection_reasons.entry(reason).or_default() += 1;
                continue;
            }
        };
        if task.rbs {
            payload.rbs_files += 1;
        } else {
            payload.ruby_files += 1;
        }
        if text.is_empty() {
            continue;
        }
        payload.documents.push(SourceFile {
            path: format!("{}/{relative}", task.document_prefix),
            text,
        });
    }
    if task.native || !payload.rejection_reasons.is_empty() {
        if payload.documents.is_empty() && !payload.rejection_reasons.is_empty() {
            payload.documents.push(SourceFile {
                path: format!("{}/unreadable-input.rb", task.document_prefix),
                text: String::new(),
            });
        }
        for document in &mut payload.documents {
            mark_namespaces_incomplete(&mut document.text);
        }
    }
    payload
}

fn mark_namespaces_incomplete(text: &mut String) {
    *text = text
        .lines()
        .map(|line| {
            if line == "end" {
                format!("include ::{OPAQUE_CONSTANTS}\nend\n")
            } else {
                format!("{line}\n")
            }
        })
        .collect();
    text.push_str(&format!(
        "class ::Object\ninclude ::{OPAQUE_CONSTANTS}\nend\n"
    ));
}

fn merge_payload(
    payload: Payload,
    documents: &mut Vec<SourceFile>,
    gems: &mut BTreeMap<String, String>,
    report: &mut Report,
) {
    for source in &payload.documents {
        gems.insert(source.path.clone(), payload.gem.clone());
    }
    documents.extend(payload.documents);
    report.ruby_files += payload.ruby_files;
    report.rbs_files += payload.rbs_files;
    for (reason, count) in payload.rejection_reasons {
        report.rejected_files += count;
        *report.rejection_reasons.entry(reason).or_default() += count;
    }
}

#[derive(Deserialize)]
struct Collection {
    gems: Vec<CollectionGem>,
}

#[derive(Deserialize)]
struct CollectionGem {
    name: String,
    version: String,
}

fn files<V: Vfs + ?Sized>(
    vfs: &V,
    root: &Path,
    extension: &str,
    errors: &mut BTreeMap<String, usize>,
) -> Vec<PathBuf> {
    if !vfs.is_dir(root) {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = match vfs.read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                *errors
                    .entry(format!(
                        "could not read declaration directory: {:?}",
                        error.kind()
                    ))
                    .or_default() += 1;
                continue;
            }
        };
        for path in entries {
            // The chosen gem/collection root may itself be a link. Interior
            // links are not followed: no cycles or escapes into other trees.
            if vfs.is_symlink(&path) {
                continue;
            }
            if vfs.is_dir(&path) {
                let excluded = path.file_name().and_then(|n| n.to_str()).is_some_and(|n| {
                    matches!(
                        n,
                        "test"
                            | "tests"
                            | "spec"
                            | "specs"
                            | "fixtures"
                            | "templates"
                            | "vendor"
                            | "node_modules"
                    )
                });
                if !excluded {
                    pending.push(path);
                }
            } else if path.extension().and_then(|e| e.to_str()) == Some(extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::{Analyzer, diagnose};
    use crate::vfs::MapVfs;

    fn fixture(body: &str, extra: &[(&str, &str)]) -> (MapVfs, App) {
        let mut tree: crate::vfs::FileTree = [
            (
                "Gemfile.lock",
                "GEM\n  specs:\n    zork (1.0)\n\nDEPENDENCIES\n  zork\n",
            ),
            (
                "config/routes.rb",
                "Rails.application.routes.draw do\nend\n",
            ),
            (
                "app/models/application_record.rb",
                "class ApplicationRecord < ActiveRecord::Base\n self.abstract_class = true\nend\n",
            ),
            (
                "app/controllers/application_controller.rb",
                "class ApplicationController < ActionController::Base\nend\n",
            ),
        ]
        .into_iter()
        .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec()))
        .collect();
        tree.insert(
            PathBuf::from("app/controllers/things_controller.rb"),
            format!(
                "class ThingsController < ApplicationController\n def show\n {body}\n end\nend\n"
            )
            .into_bytes(),
        );
        tree.extend(
            extra
                .iter()
                .map(|(p, s)| (PathBuf::from(p), s.as_bytes().to_vec())),
        );
        let vfs = MapVfs::new(tree);
        let app = super::super::app::ingest_app_with_vfs(&vfs, Path::new("")).unwrap();
        (vfs, app)
    }

    fn errors(app: &mut App) -> Vec<String> {
        Analyzer::for_source(app).analyze(app);
        diagnose(app)
            .into_iter()
            .filter(|d| d.severity == crate::diagnostic::Severity::Error)
            .map(|d| d.message)
            .collect()
    }

    fn errors_or_uncertainty(app: &mut App) -> Vec<String> {
        Analyzer::for_source(app).analyze(app);
        diagnose(app)
            .into_iter()
            .filter(|d| {
                d.severity == crate::diagnostic::Severity::Error
                    || d.message
                        .contains(" is unknown: lookup may cross incomplete namespace")
            })
            .map(|d| d.message)
            .collect()
    }

    fn parent_fixture(gem: &str, member: &str, extra: &[(&str, &str)]) -> (MapVfs, App) {
        let mut files = vec![
            ("local/gems/zork-1.0/lib/zork.rb", gem),
            ("app/lib/parent.rb", "class Parent; TOKEN = 'parent'; end"),
            (
                "app/lib/child.rb",
                "class Child < Parent; include Zork::Mix; end",
            ),
        ];
        files.extend_from_slice(extra);
        fixture(&format!("@a = Child::{member}.upcase"), &files)
    }

    #[test]
    fn review_io_failures_are_reported_and_do_not_close_surviving_namespaces() {
        struct Unreadable<'a> {
            inner: &'a MapVfs,
            failed: PathBuf,
        }
        impl Vfs for Unreadable<'_> {
            fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
                if path == self.failed {
                    Err(std::io::ErrorKind::PermissionDenied.into())
                } else {
                    self.inner.read(path)
                }
            }
            fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
                self.inner.read_to_string(path)
            }
            fn read_dir(&self, path: &Path) -> std::io::Result<Vec<PathBuf>> {
                self.inner.read_dir(path)
            }
            fn exists(&self, path: &Path) -> bool {
                self.inner.exists(path)
            }
            fn is_dir(&self, path: &Path) -> bool {
                self.inner.is_dir(path)
            }
            fn is_symlink(&self, path: &Path) -> bool {
                self.inner.is_symlink(path)
            }
        }
        let (vfs, app) = parent_fixture(
            "module Zork; module Mix; end; end",
            "TOKEN",
            &[("local/gems/zork-1.0/lib/reopen.rb", "module Zork::Mix; end")],
        );
        let mut readable = app.clone();
        load(
            &vfs,
            Path::new(""),
            &mut readable,
            &[PathBuf::from("local")],
            &[],
        );
        assert!(errors_or_uncertainty(&mut readable).is_empty());
        let broken = Unreadable {
            inner: &vfs,
            failed: "local/gems/zork-1.0/lib/reopen.rb".into(),
        };
        let mut unreadable = app;
        let report = load(
            &broken,
            Path::new(""),
            &mut unreadable,
            &[PathBuf::from("local")],
            &[],
        );
        assert_eq!(report.rejected_files, 1);
        assert!(
            report
                .rejection_reasons
                .keys()
                .any(|reason| reason.contains("PermissionDenied"))
        );
        let findings = errors_or_uncertainty(&mut unreadable);
        assert!(
            findings
                .iter()
                .any(|m| m.contains(" is unknown:") && m.contains("TOKEN")),
            "{findings:?}"
        );
    }

    #[test]
    fn review_original_core_values_and_nested_rbi_classes_survive_external_aliases() {
        for (body, gem, rbi) in [
            (
                "@a = File::NULL.no_string_method",
                "class File; NULL = Integer; end",
                "",
            ),
            (
                "@a = Zork::Client::Nested.label.no_string_method",
                "module Zork; Client = Class.new; end",
                "module Zork\nclass Client\nclass Nested\nsig { returns(String) }\ndef self.label; end\nend\nend\nend\n",
            ),
        ] {
            let (vfs, mut app) = fixture(
                body,
                &[
                    ("local/gems/zork-1.0/lib/zork.rb", gem),
                    ("sorbet/rbi/gems/zork@1.0.rbi", rbi),
                ],
            );
            assert!(
                errors(&mut app.clone())
                    .iter()
                    .any(|m| m.contains("no_string_method") && m.contains("String"))
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let after = errors(&mut app);
            assert!(
                after
                    .iter()
                    .any(|m| m.contains("no_string_method") && m.contains("String")),
                "{body}: {after:?}"
            );
        }
    }

    #[test]
    fn review_foreign_receiver_effects_stop_before_the_app_ancestor() {
        for call in [
            "Mix.const_set(:TOKEN, 42)",
            "Mix.autoload(:TOKEN, 'missing')",
            "if true; Mix.const_set(:TOKEN, 42); end",
            "HOLDER = Mix.const_set(:TOKEN, 42)",
            "unknown_receiver.const_set(:TOKEN, 42)",
            "MissingReceiver.const_set(:TOKEN, 42)",
            "Mix.class_eval('TOKEN = 42')",
            "def Mix.singleton_method_added(*); const_set(:TOKEN, 42); end; def Mix.marker; end",
            "class << Mix; def include(*); end; end",
        ] {
            let gem = format!("module Zork; module Mix; end; {call}; end");
            let (vfs, mut app) = parent_fixture(&gem, "TOKEN", &[]);
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let findings = errors_or_uncertainty(&mut app);
            assert!(
                findings
                    .iter()
                    .any(|m| m.contains(" is unknown:") && m.contains("TOKEN")),
                "{call}: {findings:?}"
            );
        }
        let (vfs, mut app) = parent_fixture(
            "module Zork; module Mix; end; Mix.const_set(:OTHER, 42); end",
            "TOKEN",
            &[],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert!(errors_or_uncertainty(&mut app).is_empty());
    }

    #[test]
    fn review_overridden_mixins_and_explicit_singleton_owners_are_incomplete() {
        for method in ["include", "prepend", "extend"] {
            for definition in [
                format!("def self.{method}(*); const_set(:TOKEN, 42); end"),
                format!("def Zork::Mix.{method}(*); const_set(:TOKEN, 42); end"),
                format!("class ::Module; def {method}(*); const_set(:TOKEN, 42); end; end"),
            ] {
                let gem = format!(
                    "module Zork; module Empty; end; module Mix; {definition}; {method} Empty; end; end"
                );
                let (vfs, mut app) = parent_fixture(&gem, "TOKEN", &[]);
                load(
                    &vfs,
                    Path::new(""),
                    &mut app,
                    &[PathBuf::from("local")],
                    &[],
                );
                let findings = errors_or_uncertainty(&mut app);
                assert!(
                    findings
                        .iter()
                        .any(|m| m.contains(" is unknown:") && m.contains("TOKEN")),
                    "{definition}: {findings:?}"
                );
            }
        }
        let (vfs, mut app) = parent_fixture(
            "module Zork; module Empty; end; module Mix; include Empty; end; end",
            "TOKEN",
            &[],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert!(errors_or_uncertainty(&mut app).is_empty());
    }

    #[test]
    fn review_rejected_reopenings_preserve_surviving_namespace_incompleteness() {
        let gem = "module Zork; module Mix; end; end";
        for (header, rejected) in [
            ("# encoding: ASCII-8BIT", true),
            ("# encoding: UTF-8", false),
        ] {
            let reopening = format!("{header}\nmodule Zork::Mix; const_set(:TOKEN, 42); end");
            let (vfs, mut app) = parent_fixture(
                gem,
                "TOKEN",
                &[("local/gems/zork-1.0/lib/reopen.rb", &reopening)],
            );
            let report = load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            assert_eq!(report.rejected_files, usize::from(rejected));
            if rejected {
                assert!(
                    report
                        .rejection_reasons
                        .keys()
                        .any(|r| r.contains("encoding"))
                );
            }
            let findings = errors_or_uncertainty(&mut app);
            assert!(
                findings
                    .iter()
                    .any(|m| m.contains(" is unknown:") && m.contains("TOKEN")),
                "{header}: {findings:?}"
            );
        }
    }

    #[test]
    fn review_transitive_lookup_consumes_resolver_completeness() {
        for (effect, expected_unknown) in [("const_set(:TOKEN, 42)", true), ("", false)] {
            let gem = format!("module Zork; module Mix; {effect}; end; end");
            let (vfs, mut app) = parent_fixture(
                &gem,
                "TOKEN",
                &[(
                    "app/lib/child.rb",
                    "class Middle < Parent; include Zork::Mix; end; class Child < Middle; end",
                )],
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let findings = errors_or_uncertainty(&mut app);
            assert_eq!(
                findings
                    .iter()
                    .any(|m| m.contains(" is unknown:") && m.contains("TOKEN")),
                expected_unknown,
                "{effect}: {findings:?}"
            );
        }
    }

    #[test]
    fn review_rbi_classes_precede_external_factory_values_and_aliases() {
        for gem in [
            "module Zork; Client = Class.new { def self.label; 'real'; end }; end",
            "module Zork; class Other; end; Client = Other; end",
        ] {
            let (vfs, mut app) = fixture(
                "@a = Zork::Client.label.no_string_method",
                &[
                    ("local/gems/zork-1.0/lib/zork.rb", gem),
                    (
                        "sorbet/rbi/gems/zork@1.0.rbi",
                        "module Zork\nclass Client\nsig { returns(String) }\ndef self.label; end\nend\nend\n",
                    ),
                ],
            );
            let before = errors(&mut app.clone());
            assert!(
                before
                    .iter()
                    .any(|m| m.contains("no_string_method") && m.contains("String")),
                "{before:?}"
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let after = errors(&mut app);
            assert!(
                after
                    .iter()
                    .any(|m| m.contains("no_string_method") && m.contains("String")),
                "{gem}: {after:?}"
            );
        }
    }

    #[test]
    fn review_inherited_external_values_keep_the_exact_rbi_declaration_model() {
        let (vfs, mut app) = fixture(
            "@a = Child::Client.label.no_string_method",
            &[
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "module Zork; module Mix; Client = Class.new; end; end",
                ),
                ("app/lib/child.rb", "class Child; include Zork::Mix; end"),
                (
                    "sorbet/rbi/gems/zork@1.0.rbi",
                    "module Zork; module Mix; class Client; sig { returns(String) }; def self.label; end; end; end; end",
                ),
            ],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        let findings = errors(&mut app);
        assert!(
            findings
                .iter()
                .any(|m| m.contains("no_string_method` on String")),
            "{findings:?}"
        );
    }

    #[test]
    fn review_external_boundaries_use_the_graphs_canonical_namespace_once() {
        let gem = "module Shared; end\nmodule Zork\nclass Shared::Client\ndef some_method; 1; end\nend\nend\n";
        for (body, phantom) in [
            ("@a = Shared::Client.new.some_method", false),
            ("@a = Zork::Shared::Client.new.some_method", true),
        ] {
            let (vfs, mut app) = fixture(body, &[("local/gems/zork-1.0/lib/zork.rb", gem)]);
            assert!(!errors(&mut app.clone()).is_empty());
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let canonical = crate::ident::ClassId(crate::ident::Symbol::new("Shared::Client"));
            let wrong = crate::ident::ClassId(crate::ident::Symbol::new("Zork::Shared::Client"));
            assert!(app.gem_boundary.classes[&canonical].declarations_only);
            assert!(!app.gem_boundary.classes.contains_key(&wrong));
            let after = errors(&mut app);
            if phantom {
                assert!(
                    after.iter().any(|m| m.contains("Zork::Shared::Client")),
                    "{after:?}"
                );
            } else {
                assert!(after.is_empty(), "the incomplete API is gradual: {after:?}");
            }
        }
    }

    #[test]
    fn dynamic_mixins_never_unblock_an_app_ancestor() {
        for effect in [
            "const_set(:TOKEN, 'gem')",
            "class_eval { TOKEN = 'gem' }",
            "autoload :TOKEN, 'a6_missing_token'",
            "HOLDER = (const_set(:TOKEN, 'gem'); 1)",
            "HOLDER ||= const_set(:TOKEN, 'gem')",
            "self::TOKEN = 'gem'",
            "class self::TOKEN; end",
            "module self::TOKEN; end",
            "class << self; Zork::Mix.const_set(:TOKEN, 'gem'); end",
            "class Host < (const_set(:TOKEN, 'gem'); Object); end",
            "def (const_set(:TOKEN, 'gem')).marker; end",
            "def self.append_features(base); const_set(:TOKEN, 'gem'); super; end",
            "def self.singleton_method_added(name); const_set(:TOKEN, 'gem'); end; def self.marker; end",
        ] {
            let gem = format!("module Zork\n module Mix\n {effect}\n end\nend\n");
            let (vfs, mut app) = fixture(
                "@value = TOKEN + 1",
                &[
                    (
                        "app/controllers/application_controller.rb",
                        "class ApplicationController < ActionController::Base\n TOKEN = 42\nend\n",
                    ),
                    (
                        "app/controllers/things_controller.rb",
                        "class ThingsController < ApplicationController\n include Zork::Mix\n def show; @value = TOKEN + 1; end\nend\n",
                    ),
                    ("local/gems/zork-1.0/lib/zork.rb", &gem),
                ],
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let after = errors_or_uncertainty(&mut app);
            assert!(
                after
                    .iter()
                    .any(|m| m.contains(" is unknown:") && m.contains("TOKEN")),
                "false binding: {effect}: {after:?}"
            );
        }
    }

    #[test]
    fn existing_rbi_types_do_not_erase_constant_uncertainty() {
        let (vfs, mut app) = fixture(
            "@value = TOKEN + 1",
            &[(
                "local/gems/zork-1.0/lib/zork.rb",
                "module Zork\n module Mix\n const_set(:TOKEN, 'gem')\n end\nend\n",
            )],
        );
        let id = crate::ident::ClassId(crate::ident::Symbol::new("Zork::Mix"));
        // The existing boundary represents a previously read signature.
        app.gem_boundary
            .classes
            .entry(id.clone())
            .or_default()
            .instance_methods
            .insert(
                crate::ident::Symbol::new("typed_method"),
                crate::ty::Ty::Str,
            );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        let mix = &app.gem_boundary.classes[&id];
        assert!(mix.constant_namespace_incomplete);
        assert_eq!(
            mix.instance_methods[&crate::ident::Symbol::new("typed_method")],
            crate::ty::Ty::Str
        );
    }

    #[test]
    fn a_reopened_core_ancestor_cannot_expose_a_modeled_global_namesake() {
        let (vfs, mut app) = fixture(
            "@value = Child.new.value",
            &[
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "class StandardError\n const_set(:TokenGlobal, 'gem')\nend\n",
                ),
                (
                    "app/lib/token_global.rb",
                    "class TokenGlobal\n def self.answer; 42; end\nend\n",
                ),
                (
                    "app/lib/child.rb",
                    "class Child < StandardError\n def value; TokenGlobal.answer; end\nend\n",
                ),
            ],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        let after = errors_or_uncertainty(&mut app);
        assert!(
            after
                .iter()
                .any(|m| m.contains(" is unknown:") && m.contains("TokenGlobal")),
            "{after:?}"
        );
    }

    #[test]
    fn an_own_constant_precedes_an_incomplete_ancestor() {
        let (vfs, mut app) = fixture(
            "@value = Events::TOKEN + 1",
            &[
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "module Zork\n module Mix\n const_set(:TOKEN, 'gem')\n end\nend\n",
                ),
                (
                    "app/lib/events.rb",
                    "module Events\n TOKEN = 42\n include Zork::Mix\nend\n",
                ),
            ],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert!(!errors(&mut app).iter().any(|m| m.contains("Events::TOKEN")));
    }

    #[test]
    fn repeated_root_effects_share_one_unresolvable_barrier() {
        let (vfs, mut app) = fixture(
            "@value = Zork::Missing",
            &[
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "require 'other'\nmodule Zork; end\n",
                ),
                (
                    "local/gems/zork-1.0/lib/other.rb",
                    "autoload :Other, 'other_impl'\n",
                ),
                (
                    "app/lib/marker.rb",
                    "module RoundhouseIncompleteConstantsA6; end\n",
                ),
            ],
        );
        let inputs = Inputs {
            gem_paths: vec![PathBuf::from("local")],
            ..Default::default()
        };
        let docs = prepare(&vfs, Path::new(""), &mut app, &inputs);
        let root_barrier = format!("class ::Object\ninclude ::{OPAQUE_CONSTANTS}\nend\n");
        assert_eq!(
            docs.iter()
                .map(|d| d.text.matches(&root_barrier).count())
                .sum::<usize>(),
            1
        );
        app.const_resolver =
            crate::analyze::PreparedConstResolver::with_external_declarations(&app.sources, docs);
        assert!(!errors_or_uncertainty(&mut app).is_empty());
    }

    #[test]
    fn may_definitions_do_not_become_positive_facts() {
        for gem in [
            "if false\n module Zork\n class Ghost; end\n end\nend\n",
            "unless true\n module Zork\n class Ghost; end\n end\nend\n",
            "module Zork\n if RUBY_ENGINE == 'jruby'\n class Ghost; end\n end\nend\n",
            "module Zork\n while false\n class Ghost; end\n end\nend\n",
            "module Zork\n begin\n 1\n rescue StandardError\n class Ghost; end\n end\nend\n",
            "module Zork; end\nEND { class Zork::Ghost; end }\n",
            "raise 'stop'\nmodule Zork\n class Ghost; end\nend\n",
            "HOLDER = raise('stop')\nmodule Zork\n class Ghost; end\nend\n",
            "::Kernel.raise('stop')\nmodule Zork\n class Ghost; end\nend\n",
            "HOLDER = identity(raise('stop'))\nmodule Zork\n class Ghost; end\nend\n",
            "class StopHere < raise('stop'); end\nmodule Zork\n class Ghost; end\nend\n",
            "class StopHere\n raise 'stop'\nend\nmodule Zork\n class Ghost; end\nend\n",
            "module StopHere\n Kernel.exit(1)\nend\nmodule Zork\n class Ghost; end\nend\n",
            "module Zork\n Ghost &&= 42\nend\n",
            "module Zork\n Ghost += 42\nend\n",
            "module Zork; end\nZork::Ghost &&= 42\n",
            "module Zork; end\nZork::Ghost += 42\n",
        ] {
            let (vfs, mut app) = fixture(
                "@value = Zork::Ghost.new",
                &[("local/gems/zork-1.0/lib/zork.rb", gem)],
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            assert!(
                !errors_or_uncertainty(&mut app).is_empty(),
                "false clean: {gem}"
            );
        }
        for gem in [
            "module Zork\n module Mix; TOKEN = 42; end\n class Host\n include Mix if false\n end\nend\n",
            "module ReviewMix; TOKEN = 42; end\nclass String\n include ReviewMix if false\nend\n",
        ] {
            let read = if gem.contains("class String") {
                "String::TOKEN"
            } else {
                "Zork::Host::TOKEN"
            };
            let (vfs, mut app) = fixture(
                &format!("@value = {read}"),
                &[("local/gems/zork-1.0/lib/zork.rb", gem)],
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            assert!(
                !errors_or_uncertainty(&mut app).is_empty(),
                "false ancestry: {gem}"
            );
        }
    }

    #[test]
    fn multiarg_mixins_keep_constant_and_method_priority() {
        let app_mixins = "module FirstMix\n TOKEN = 'app'\n def choose; 'app'; end\nend\nmodule SecondMix\n TOKEN = 42\n def choose; 42; end\nend\n";
        let signatures = "module FirstMix\n def choose: () -> String\nend\nmodule SecondMix\n def choose: () -> Integer\nend\n";
        for operation in ["include", "prepend", "extend"] {
            let gem = format!(
                "module Zork\n class Host\n {operation} ::FirstMix, ::SecondMix\n end\nend\n"
            );
            assert!(
                ruby_declarations(gem.as_bytes())
                    .unwrap()
                    .contains(&format!("{operation} ::FirstMix, ::SecondMix"))
            );
            let recv = if operation == "extend" {
                "Zork::Host"
            } else {
                "Zork::Host.new"
            };
            let body = format!("@value = {recv}.choose.no_string_method");
            let (vfs, mut app) = fixture(
                &body,
                &[
                    ("local/gems/zork-1.0/lib/zork.rb", &gem),
                    ("app/lib/mixes.rb", app_mixins),
                    ("sig/mixes.rbs", signatures),
                ],
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let diagnostics = errors(&mut app);
            assert!(
                diagnostics
                    .iter()
                    .any(|m| m.contains("no_string_method") && m.contains("String")),
                "{operation}: {diagnostics:?}"
            );
            if operation != "extend" {
                let (vfs, mut app) = fixture(
                    "@value = Zork::Host::TOKEN.upcase",
                    &[
                        ("local/gems/zork-1.0/lib/zork.rb", &gem),
                        ("app/lib/mixes.rb", app_mixins),
                    ],
                );
                load(
                    &vfs,
                    Path::new(""),
                    &mut app,
                    &[PathBuf::from("local")],
                    &[],
                );
                assert!(
                    errors(&mut app).is_empty(),
                    "{operation} bound TOKEN to Integer"
                );
            }
        }
        let text = ruby_declarations(
            b"module Zork\n class Host\n include FirstMix, dynamic_mixin\n end\nend\n",
        )
        .unwrap();
        assert!(!text.contains("include FirstMix"));
        assert!(text.contains(OPAQUE_CONSTANTS));
    }

    #[test]
    fn directory_prefixes_are_not_release_or_platform_evidence() {
        for directory in [
            "zork-2.0",
            "zork-1.0-beta",
            "zork-1.0-java",
            "zork-1.0-arm64-darwin",
        ] {
            let path = format!("local/gems/{directory}/lib/zork.rb");
            let (vfs, mut app) = fixture(
                "@value = Zork::Ghost.new",
                &[(&path, "module Zork\n class Ghost; end\nend\n")],
            );
            let report = load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            assert_eq!(report.gems_found, 0, "{directory}");
            assert!(!errors(&mut app).is_empty(), "admitted {directory}");
        }
    }

    #[test]
    fn ambiguous_membership_on_a_declaration_only_instance_stays_gradual() {
        let (vfs, mut app) = fixture(
            "@a = Zork::Bag.new.include?(1)",
            &[(
                "local/gems/zork-1.0/lib/zork.rb",
                "module Zork\n class Bag < ChosenAtRuntime\n def include?(value); dangerous(); end\n end\nend\n",
            )],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert!(errors(&mut app).is_empty());
    }

    #[test]
    fn app_implicit_directory_namespace_wins_over_an_imported_ancestor_name() {
        let (vfs, mut app) = fixture(
            "@a = Category::Query::Search.new",
            &[
                (
                    "app/models/category.rb",
                    "class Category < ActiveRecord::Base\nend\n",
                ),
                (
                    "app/services/category/query/search.rb",
                    "class Category::Query::Search\nend\n",
                ),
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "module ActiveRecord\n module AttributeMethods\n module Query\n end\n end\n class Base\n include AttributeMethods\n end\nend\n",
                ),
            ],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        let after = errors(&mut app);
        assert!(
            !after.iter().any(|m| m.contains("Category::Query::Search")),
            "{after:?}"
        );
    }

    #[test]
    fn an_explicit_gem_class_is_not_reopened_as_an_implicit_app_module() {
        let (vfs, mut app) = fixture(
            "@a = ConnectionPool::Shared.new.check({})",
            &[
                (
                    "app/lib/connection_pool/shared.rb",
                    "class ConnectionPool::Shared < ConnectionPool\n def check(value)\n value.is_a?(Hash)\n end\nend\n",
                ),
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "class ConnectionPool\nend\n",
                ),
            ],
        );
        let inputs = Inputs {
            gem_paths: vec![PathBuf::from("local")],
            ..Default::default()
        };
        let docs = prepare(&vfs, Path::new(""), &mut app, &inputs);
        assert!(
            !docs
                .iter()
                .any(|doc| doc.text.contains("module ::ConnectionPool\n"))
        );
        app.const_resolver =
            crate::analyze::PreparedConstResolver::with_external_declarations(&app.sources, docs);
        assert!(errors(&mut app).is_empty());
    }

    #[test]
    fn extraction_does_not_copy_executable_bodies_or_dynamic_scopes() {
        let source = b"module Zork\n VALUE = dangerous()\n class Base < ::StandardError\n end\n def self.boot\n Hidden = 1\n end\n class_eval do\n WrongOwner = 1\n end\nend\n";
        // Ruby disallows constant assignment inside a def. The entire invalid
        // file is skipped; parser recovery must never invent declarations.
        assert!(ruby_declarations(source).is_err());
        let source = b"module Zork\n VALUE = dangerous()\n class Base < ::StandardError\n end\n def self.boot; dangerous(); end\n class_eval do\n WrongOwner = 1\n end\nend\n";
        let text = ruby_declarations(source).unwrap();
        assert!(text.contains("VALUE = nil"));
        assert!(text.contains("class Base < ::StandardError"));
        assert!(
            !text.contains("dangerous") && !text.contains("WrongOwner") && !text.contains("def ")
        );
    }

    #[test]
    fn source_declarations_resolve_aliases_ancestry_values_and_preserve_real_misses() {
        let (vfs, mut app) = fixture(
            "@a = [Zork::Child, Zork::TOKEN, TOP_TOKEN, Alias::Nested, Zork::Child::INHERITED, Zork::Child::MIXED, Zork::Missing]",
            &[
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "module Zork\n TOKEN = explode()\n module Mix\n MIXED = 1\n end\n class Base\n INHERITED = 1\n end\n class Child < Base\n include Mix\n class Nested\n end\n end\nend\nAlias = Zork::Child\nTOP_TOKEN = explode()\n",
                ),
                (
                    "local/gems/zork-2.0/lib/zork.rb",
                    "module Zork\n class Missing\n end\nend\n",
                ),
                (
                    "local/gems/unlocked-1.0/lib/x.rb",
                    "module Zork\n class Missing\n end\nend\n",
                ),
            ],
        );
        let source_count = app.sources.len();
        let before = errors(&mut app.clone());
        assert_eq!(
            before
                .iter()
                .filter(|m| m.starts_with("constant not supported"))
                .count(),
            7,
            "{before:?}"
        );
        let report = load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert_eq!(report.gems_found, 1);
        let after = errors_or_uncertainty(&mut app);
        assert_eq!(after.len(), 1, "{after:?}");
        assert!(after[0].contains("Zork::Missing"));
        assert_eq!(app.sources.len(), source_count);
        assert!(app.gem_boundary.loaded.is_empty());
        assert!(
            !app.library_classes
                .iter()
                .any(|c| c.name.0.as_str() == "Zork::Child")
        );
    }

    #[test]
    fn community_rbs_uses_the_installed_collection_selection_and_no_type_alias_constants() {
        let (vfs, mut app) = fixture(
            "@a = [Zork::FromRbs, Zork::FromRbs::VALUE, Zork::WrongVersion]",
            &[
                (
                    "rbs_collection.lock.yaml",
                    "path: .rbs_collection\ngems:\n- name: zork\n  version: '0.9'\n",
                ),
                (
                    ".rbs_collection/zork/0.9/main.rbs",
                    "module Zork\n class FromRbs\n VALUE: String\n end\n type opaque = String\n interface _Protocol\n end\nend\n",
                ),
                (
                    ".rbs_collection/zork/1.0/main.rbs",
                    "module Zork\n class WrongVersion\n end\nend\n",
                ),
            ],
        );
        let report = load(&vfs, Path::new(""), &mut app, &[], &[]);
        assert_eq!(report.rbs_files, 0);
        assert_eq!(report.collection_contracts_skipped, 1);
        assert_eq!(report.collection_version_conflicts, 1);
        let report = load(
            &vfs,
            Path::new(""),
            &mut app,
            &[],
            &[PathBuf::from(".rbs_collection/zork/0.9")],
        );
        assert_eq!(report.trusted_rbs_inputs, 1);
        assert_eq!(report.rbs_files, 1);
        let after = errors(&mut app);
        assert_eq!(after.len(), 1, "{after:?}");
        assert!(after[0].contains("Zork::WrongVersion"));
        assert!(
            !app.gem_boundary
                .classes
                .keys()
                .any(|c| c.0.as_str().contains("opaque") || c.0.as_str().contains("Protocol"))
        );
    }

    #[test]
    fn app_values_and_tapioca_method_types_keep_precedence() {
        let (vfs, mut app) = fixture(
            "@a = Zork::TOKEN.no_string_method\n@b = Zork::Child.label.no_string_method",
            &[
                ("app/lib/zork.rb", "module Zork\n TOKEN = 'app'\nend\n"),
                (
                    "sorbet/rbi/gems/zork@1.0.rbi",
                    "module Zork\n class Child\n sig { returns(String) }\n def self.label; end\n end\nend\n",
                ),
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "module Zork\n TOKEN = 42\n class Child\n def self.label; 42; end\n end\nend\n",
                ),
            ],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        let after = errors(&mut app);
        assert_eq!(
            after
                .iter()
                .filter(|m| m.contains("no_string_method") && m.contains("String"))
                .count(),
            2,
            "{after:?}"
        );
    }

    #[test]
    fn literal_named_effects_do_not_poison_unrelated_names_or_inherit_marker_constants() {
        for effect in ["const_set(:OTHER, 1)", "autoload :OTHER, 'missing_other'"] {
            let gem = format!("module Zork\nmodule Mix\n{effect}\nend\nend\n");
            let (vfs, mut app) = fixture(
                "@value = TOKEN + 1",
                &[
                    ("local/gems/zork-1.0/lib/zork.rb", &gem),
                    (
                        "app/controllers/application_controller.rb",
                        "class ApplicationController < ActionController::Base\nTOKEN = 42\nend\n",
                    ),
                    (
                        "app/controllers/things_controller.rb",
                        "class ThingsController < ApplicationController\ninclude Zork::Mix\ndef show; @value = TOKEN + 1; end\nend\n",
                    ),
                    (
                        "app/lib/spoof.rb",
                        "module RoundhouseUncertainNameA8_4f54484552\nTOKEN = 'wrong'\nend\n",
                    ),
                ],
            );
            load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            let after = errors_or_uncertainty(&mut app);
            assert!(after.is_empty(), "{effect}: {after:?}");
            let header = ruby_declarations(gem.as_bytes()).unwrap();
            assert!(header.contains("RoundhouseUncertainNameA8_4f54484552"));
            assert!(!header.contains(OPAQUE_CONSTANTS));
        }
        assert!(constant_effect("RoundhouseUncertainNameA8_名字").is_none());
        for statement in [
            "const_set(dynamic_name, 1)",
            "const_set(:OTHER, const_set(:TOKEN, 'gem'))",
            "def self.const_set(*); super(:TOKEN, 'gem'); end\nconst_set(:OTHER, 1)",
        ] {
            let text =
                ruby_declarations(format!("module Zork\n{statement}\nend\n").as_bytes()).unwrap();
            assert!(text.contains(OPAQUE_CONSTANTS), "{statement}");
        }
    }

    #[test]
    fn a_literal_receiver_is_not_proof_that_an_overridable_value_call_is_pure() {
        let text=ruby_declarations(b"class Array\n def freeze; Zork::Mix.const_set(:TOKEN, 'gem'); self; end\nend\nmodule Zork\nmodule Mix\nHOLDER = [].freeze\nend\nend\n").unwrap();
        assert!(text.contains(OPAQUE_CONSTANTS));
    }

    #[test]
    fn a_nil_constant_effect_does_not_claim_that_the_source_dependency_loaded() {
        let (vfs, mut app) = fixture(
            "@value = TOKEN + 1",
            &[
                (
                    "local/gems/zork-1.0/lib/zork.rb",
                    "module Zork\nmodule Mix\nattr_reader nil\nend\nend\n",
                ),
                (
                    "app/controllers/application_controller.rb",
                    "class ApplicationController < ActionController::Base\nTOKEN = 42\nend\n",
                ),
                (
                    "app/controllers/things_controller.rb",
                    "class ThingsController < ApplicationController\ninclude Zork::Mix\ndef show; @value = TOKEN + 1; end\nend\n",
                ),
            ],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert!(errors(&mut app).is_empty());
        assert!(
            diagnose(&app)
                .iter()
                .any(|d| d.code() == "constant_loading_unverified"
                    && d.severity == crate::diagnostic::Severity::Warning
                    && d.message.contains("Zork::Mix"))
        );
    }

    #[test]
    fn declaration_only_calls_do_not_poison_missing_constant_lookup() {
        for statement in [
            "protected",
            "private",
            "public",
            "module_function",
            "attr_reader :label",
            "attr_accessor :label",
            "delegate :label, to: :record",
            "class_attribute :settings",
            "mattr_accessor :settings",
            "private def label; 1; end",
            "extend ActiveSupport::Concern\nincluded do\nattr_reader :label\nprivate\ndef label; 1; end\nend",
            "extend ActiveSupport::Concern\nincluded do\nprivate if respond_to?(:private)\nend",
        ] {
            let text = ruby_declarations(
                format!("module Zork\nmodule Mix\n{statement}\nend\nend\n").as_bytes(),
            )
            .unwrap();
            assert!(!text.contains(OPAQUE_CONSTANTS), "{statement}: {text}");
        }
    }

    #[test]
    fn declaration_call_arguments_blocks_overrides_and_hooks_keep_uncertainty() {
        for statement in [
            "attr_reader(const_set(:TOKEN, 'gem'))",
            "private(class_eval { TOKEN = 'gem' })",
            "delegate :label, to: const_set(:TOKEN, 'gem')",
            "def self.private; const_set(:TOKEN, 'gem'); end\nprivate",
            "def self.method_added(name); const_set(:TOKEN, 'gem'); end\nattr_reader :label",
            "alias_method :private, :dynamic_visibility\nprivate",
            "extend CustomVisibility\nprivate",
            "extend ActiveSupport::Concern\nincluded do\nconst_set(:TOKEN, 'gem')\nend",
            "extend ActiveSupport::Concern\nincluded do\nTOKEN = 'gem' if false\nend",
            "extend ActiveSupport::Concern\nincluded do\nclass_eval { TOKEN = 'gem' }\nend",
            // Rails helper/helper_method may create HelperMethods, so are not nil-effect contracts.
            "extend ActiveSupport::Concern\nincluded do\nhelper_method :label\nend",
        ] {
            let text = ruby_declarations(
                format!("module Zork\nmodule Mix\n{statement}\nend\nend\n").as_bytes(),
            )
            .unwrap();
            assert!(text.contains(OPAQUE_CONSTANTS), "{statement}: {text}");
        }
    }

    #[test]
    fn an_exact_native_platform_source_is_rejected() {
        let (vfs, mut app) = fixture(
            "@value = Child.new.value",
            &[
                (
                    "Gemfile.lock",
                    "GEM\n  specs:\n    zork (1.0-arm64-darwin)\n\nPLATFORMS\n  arm64-darwin\n\nDEPENDENCIES\n  zork\n",
                ),
                (
                    "local/gems/zork-1.0-arm64-darwin/lib/zork.rb",
                    "module Zork\nmodule Mix; end\nend\n",
                ),
                (
                    "app/lib/child.rb",
                    "class Parent\n TOKEN = 42\nend\nclass Child < Parent\ninclude Zork::Mix\ndef value; TOKEN + 1; end\nend\n",
                ),
            ],
        );
        let report = load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert_eq!(report.gems_found, 0);
        assert_eq!(report.non_ruby_specs_rejected, 1);
        assert!(
            !app.gem_boundary
                .classes
                .values()
                .any(|c| c.declarations_only)
        );
    }

    #[test]
    fn bundle_path_discovery_and_bare_ancestor_reads_need_no_gem_execution() {
        let (vfs, mut app) = fixture(
            "@a = MyChild.new.value",
            &[
                (".bundle/config", "BUNDLE_PATH: local\n"),
                (
                    "local/ruby/4.0.0/gems/zork-1.0/lib/zork.rb",
                    "module Zork\n module Mix\n end\nend\nabort 'must never run'\n",
                ),
                // A real installed native gem is ineligible for automatic source
                // compilation, but its Ruby declarations remain useful to check.
                ("local/ruby/4.0.0/gems/zork-1.0/ext/native.c", ""),
                (
                    "lib/my_child.rb",
                    "class MyBase\n LIMIT = 17\nend\nclass MyChild < MyBase\n include Zork::Mix\n def value\n LIMIT\n end\nend\n",
                ),
            ],
        );
        let before = errors(&mut app.clone());
        assert!(before.iter().any(|m| m.contains("LIMIT")), "{before:?}");
        let source_count = app.sources.len();
        let report = load(&vfs, Path::new(""), &mut app, &[], &[]);
        assert_eq!(report.gems_found, 1);
        assert!(
            errors_or_uncertainty(&mut app)
                .iter()
                .any(|m| m.contains("LIMIT") && m.contains("Zork::Mix"))
        );
        assert_eq!(source_count, app.sources.len());
        // Public source edits invalidate cached answers without losing the
        // separately retained external declaration inputs.
        app.sources[0].text.push('\n');
        assert!(
            errors_or_uncertainty(&mut app)
                .iter()
                .any(|m| m.contains("LIMIT") && m.contains("Zork::Mix"))
        );
    }

    #[test]
    fn external_core_declarations_preserve_the_modeled_literal_type() {
        let (vfs, mut app) = fixture(
            "@a = Float::INFINITY.no_float_method",
            &[("explicit/core.rbs", "class Float\n INFINITY: Float\nend\n")],
        );
        let before = errors(&mut app.clone());
        assert_eq!(before.len(), 1);
        assert!(before[0].contains("no_float_method") && before[0].contains("Float"));
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[],
            &[PathBuf::from("explicit/core.rbs")],
        );
        assert_eq!(errors(&mut app), before);
    }

    #[test]
    fn a_fresh_class_under_a_core_namespace_keeps_an_incomplete_api_gradual() {
        let (vfs, mut app) = fixture(
            "@a = JSON::LD::API.unmodeled_api",
            &[(
                "local/gems/zork-1.0/lib/zork.rb",
                "module JSON\n module LD\n class API\n end\n end\nend\n",
            )],
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert!(errors(&mut app).is_empty());
    }

    #[test]
    fn declarations_do_not_ship_a_gem_helper_implementation() {
        let (vfs, mut app) = fixture(
            "@a = authorize(1)",
            &[
                (
                    "Gemfile.lock",
                    "GEM\n  specs:\n    pundit (2.5.2)\n\nDEPENDENCIES\n  pundit\n",
                ),
                (
                    "local/gems/pundit-2.5.2/lib/pundit.rb",
                    "module Pundit\n module Authorization\n def authorize(record); record; end\n end\nend\n",
                ),
                (
                    "app/controllers/things_controller.rb",
                    "class ThingsController < ApplicationController\n include Pundit::Authorization\n def show\n authorize(1)\n end\nend\n",
                ),
            ],
        );
        let before = errors(&mut app.clone());
        assert!(
            before
                .iter()
                .any(|m| m.contains("authorize") && m.contains("pundit")),
            "{before:?}"
        );
        load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        let after = errors(&mut app);
        assert!(
            after
                .iter()
                .any(|m| m.contains("authorize") && m.contains("pundit")),
            "{after:?}"
        );
        assert!(app.gem_boundary.loaded.is_empty());
    }

    #[test]
    fn explicit_rbs_roots_with_the_same_filename_keep_distinct_documents() {
        let (vfs, mut app) = fixture(
            "@value = [FirstContract, SecondContract]",
            &[
                ("first/shared.rbs", "class FirstContract\nend\n"),
                ("second/shared.rbs", "class SecondContract\nend\n"),
            ],
        );
        let inputs = Inputs {
            rbs_paths: vec![PathBuf::from("first"), PathBuf::from("second")],
            ..Default::default()
        };
        let documents = prepare(&vfs, Path::new(""), &mut app, &inputs);
        assert_eq!(documents.len(), 2);
        assert_ne!(documents[0].path, documents[1].path);
        app.const_resolver = crate::analyze::PreparedConstResolver::with_external_declarations(
            &app.sources,
            documents,
        );
        assert!(errors(&mut app).is_empty());
    }

    #[test]
    fn git_path_and_sourced_gems_do_not_supply_external_declarations() {
        for lock in [
            "GIT\n  remote: https://example.invalid/zork.git\n  revision: 1111111111111111111111111111111111111111\n  specs:\n    zork (1.0)\n\nDEPENDENCIES\n  zork!\n",
            "PATH\n  remote: local/zork\n  specs:\n    zork (1.0)\n\nDEPENDENCIES\n  zork!\n",
        ] {
            let (vfs, mut app) = fixture(
                "@value = Zork::Client",
                &[(
                    "local/gems/zork-1.0/lib/zork.rb",
                    "module Zork\nclass Client; end\nend\n",
                )],
            );
            app.gem_lock = Some(crate::gems::Lockfile::parse(lock));
            let report = load(
                &vfs,
                Path::new(""),
                &mut app,
                &[PathBuf::from("local")],
                &[],
            );
            assert_eq!(report.gems_found, 0);
            assert!(
                !app.gem_boundary
                    .classes
                    .values()
                    .any(|c| c.declarations_only)
            );
        }
        let (vfs, mut app) = fixture(
            "@value = Zork::Client",
            &[(
                "local/gems/zork-1.0/lib/zork.rb",
                "module Zork\nclass Client; end\nend\n",
            )],
        );
        app.gem_lock.as_mut().unwrap().source.push("zork".into());
        let report = load(
            &vfs,
            Path::new(""),
            &mut app,
            &[PathBuf::from("local")],
            &[],
        );
        assert_eq!(report.gems_found, 0);
    }
}
