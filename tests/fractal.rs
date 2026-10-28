//! `--target fractal` (`emit::fractal`): the driver every Fractal rewrite
//! goes through.
//!
//! What it promises: the output holds only changed files at app-relative
//! paths; only files named with `--file` are compiled; edits keep every
//! line where it was and leave the file parsing as it did, or the whole
//! proposal is refused and reported. A test pass that swaps literal text
//! makes rewrites observable.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::fractal::{self, Context, Edit, MANIFEST, Pass, Plan, Proposal, Scope, Unit};
use roundhouse::ingest::ingest_app;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "accounts", force: :cascade do |t|
    t.string "name"
  end
end
"#;

const CURRENT: &str = r#"# frozen_string_literal: true

class Current < ActiveSupport::CurrentAttributes
  attribute :user

  def account
    Account.first
  end

  def label
    "original"
  end

  private def secret
    "original"
  end

  def self.build
    new
  end

  class << self
    def other
      "original"
    end
  end

  class Settings
    def account
      Account.first
    end
  end
end
"#;

/// A reopening in another file is the same class.
const CURRENT_REOPENED: &str = r#"class Current
  def room
    @room
  end
end
"#;

/// Definitions an overlay could not replace; rewritten in place they are
/// ordinary source.
const SHAPES: &str = r#"class Shapes
  def plain
    "original"
  end

  [1].each do
    def in_a_block; end
  end

  memoize def wrapped
    "original"
  end

  def located
    __dir__
  end

  def defines
    def inner; end
  end
end
"#;

/// Definitions on another object. Ingest records them as a `--survey`
/// gap, so the file is untrusted too; the method's own reason wins.
const FOREIGN: &str = r#"class Foreign
  def plain
    "original"
  end

  def OTHER.elsewhere; end

  class << OTHER
    def also_elsewhere; end
  end
end
"#;

/// Nested classes: a method belongs to the innermost one around it.
const NESTED: &str = r#"class Outer
  def outer_label
    "original"
  end

  class Middle
    def middle_label
      "original"
    end

    class Inner
      def inner_label
        "original"
      end
    end
  end
end
"#;

const REPORT: &str = r#"# frozen_string_literal: true

module Admin
  NAME = "-report"

  class Report
    def title
      "original" + NAME
    end
  end
end
"#;

fn base_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
        ("app/models/current.rb", CURRENT),
        ("app/models/current/reopened.rb", CURRENT_REOPENED),
        ("app/models/shapes.rb", SHAPES),
        ("app/models/outer.rb", NESTED),
        ("app/models/admin/report.rb", REPORT),
    ]
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-fractal-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn app_dir(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = scratch(name).join("app");
    for (path, content) in files {
        let path = dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    dir
}

fn build_plan(dir: &Path, files: &[&str], passes: &[&dyn Pass]) -> Result<Plan, String> {
    let mut app = ingest_app(dir).map_err(|e| e.to_string())?;
    Analyzer::new(&app).analyze(&mut app);
    let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
    fractal::plan(&app, dir, &files, &[], passes)
}

fn method<'r>(plan: &'r Plan, key_prefix: &str) -> &'r fractal::MethodReport {
    plan.report
        .methods
        .iter()
        .find(|m| m.key.starts_with(key_prefix))
        .unwrap_or_else(|| panic!("no method {key_prefix} in {:?}", plan.report.methods.iter().map(|m| &m.key).collect::<Vec<_>>()))
}

fn keys(plan: &Plan) -> Vec<String> {
    plan.report.methods.iter().map(|m| m.key.split('@').next().unwrap().to_string()).collect()
}

/// Every file under `dir`, relative to it.
fn files_in(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// Swaps every occurrence of `from` in the unit's file for `to`. A test fixture that makes a rewrite observable, not an
/// optimization.
struct Swap {
    from: &'static str,
    to: &'static str,
}

impl Swap {
    fn new(from: &'static str, to: &'static str) -> Swap {
        Swap { from, to }
    }
}

impl Pass for Swap {
    fn name(&self) -> &'static str {
        "test-swap"
    }

    fn scope(&self) -> Scope {
        Scope::Files(Vec::new())
    }

    fn run(&self, _context: &Context<'_>, unit: &Unit<'_>) -> Proposal {
        let edits = unit
            .text
            .match_indices(self.from)
            .map(|(at, _)| Edit { span: unit.span(at, at + self.from.len()), text: self.to.to_string() })
            .collect();
        Proposal { edits, declined: Vec::new() }
    }
}

/// Proposes whatever edits it is given, for the driver's refusals.
struct Fixed(Box<dyn Fn(&Unit<'_>) -> Vec<Edit>>);

impl Pass for Fixed {
    fn name(&self) -> &'static str {
        "test-fixed"
    }

    fn scope(&self) -> Scope {
        Scope::Files(Vec::new())
    }

    fn run(&self, _context: &Context<'_>, unit: &Unit<'_>) -> Proposal {
        Proposal { edits: (self.0)(unit), ..Proposal::default() }
    }
}

/// An edit replacing the first `needle` in the unit's text.
fn at(unit: &Unit<'_>, needle: &str, text: &str) -> Edit {
    let start = unit.text.find(needle).unwrap_or_else(|| panic!("{needle} not in {}", unit.path));
    Edit { span: unit.span(start, start + needle.len()), text: text.to_string() }
}

const CURRENT_RB: &str = "app/models/current.rb";
const OUTER_RB: &str = "app/models/outer.rb";

#[test]
fn no_file_or_no_pass_emits_no_app_files() {
    let dir = app_dir("identity", &base_files());
    for (name, files) in [("none", vec![]), ("selected", vec![CURRENT_RB, "app/models/shapes.rb"])] {
        let plan = build_plan(&dir, &files, &[]).unwrap();
        assert!(plan.rewritten.is_empty() && plan.report.rewrites.is_empty(), "{name}");
        let out = scratch(&format!("identity-out-{name}")).join("out");
        let written = fractal::write(&dir, &out, &plan).unwrap();
        assert_eq!(written.rewritten, 0, "{name}");
        assert_eq!(files_in(&out), [MANIFEST], "{name}");
        for (path, content) in base_files() {
            assert_eq!(fs::read_to_string(dir.join(path)).unwrap(), content, "{name}: {path}");
        }
    }
}

#[test]
fn every_method_in_a_compiled_file_is_reported_and_nothing_else() {
    let dir = app_dir("selection", &base_files());
    let plan = build_plan(&dir, &[CURRENT_RB], &[]).unwrap();
    // The nested class's method is in the file; the reopening is not.
    assert_eq!(
        keys(&plan),
        ["Current#account", "Current#label", "Current#secret", "Current.build", "Current.other", "Current::Settings#account"],
    );
    assert_eq!(plan.report.compiled, [CURRENT_RB]);
}

#[test]
fn a_method_belongs_to_the_innermost_class_around_it_and_any_line_can_be_rewritten() {
    let dir = app_dir("nesting", &base_files());
    let plan = build_plan(&dir, &[OUTER_RB], &[]).unwrap();
    assert_eq!(keys(&plan), ["Outer#outer_label", "Outer::Middle#middle_label", "Outer::Middle::Inner#inner_label"]);
    let swap = Swap::new("\"original\"", "\"rewritten\"");
    let plan = build_plan(&dir, &[OUTER_RB], &[&swap]).unwrap();
    assert_eq!(plan.rewritten[0].1, NESTED.replace("\"original\"", "\"rewritten\""));
}

#[test]
fn a_file_the_build_did_not_read_is_an_error() {
    let dir = app_dir("unknown", &base_files());
    let err = build_plan(&dir, &["app/models/curent.rb"], &[]).err().expect("a misspelt file must not build the identity");
    assert!(err.contains("--file app/models/curent.rb: not a Ruby source the build read"), "{err}");
}

#[test]
fn any_definition_can_be_rewritten_in_place() {
    let dir = app_dir("shapes", &base_files());
    let swap = Swap::new("\"original\"", "\"rewritten\"");
    let plan = build_plan(&dir, &["app/models/shapes.rb"], &[&swap]).unwrap();
    // Inside a `memoize` argument too: the rewritten source is what runs.
    assert_eq!(plan.report.rewrites.len(), 2, "{:?}", plan.report.rewrites);
    assert!(plan.report.declined.is_empty(), "{:?}", plan.report.declined);
    assert_eq!(plan.rewritten[0].1, SHAPES.replace("\"original\"", "\"rewritten\""));
}

#[test]
fn knowledge_reports_types_and_effects() {
    let dir = app_dir("knowledge", &base_files());
    let plan = build_plan(&dir, &[CURRENT_RB, "app/models/current/reopened.rb"], &[]).unwrap();
    let label = &method(&plan, "Current#label").knowledge;
    assert_eq!((label.copies, label.typed, label.expressions), (1, 1, 1));
    assert!(label.no_recorded_effects && !label.writes_state, "{label:?}");
    // `Account.first` reads the database.
    assert!(!method(&plan, "Current#account").knowledge.no_recorded_effects);
    // An ivar read the analyzer cannot type.
    let room = &method(&plan, "Current#room").knowledge;
    assert!(room.typed < room.expressions, "{room:?}");
}

#[test]
fn the_overlay_contains_only_rewritten_files_and_keeps_lines() {
    let dir = app_dir("rewrite", &base_files());
    let swap = Swap::new("\"original\" + NAME", "NAME.dup.prepend(\"rewritten\")");
    let plan = build_plan(&dir, &["app/models/admin/report.rb"], &[&swap]).unwrap();
    assert_eq!(plan.report.rewritten, ["app/models/admin/report.rb"]);
    let r = &plan.report.rewrites[0];
    assert_eq!((r.at.as_str(), r.pass.as_str(), r.from.as_str()), ("app/models/admin/report.rb:8:7", "test-swap", "\"original\" + NAME"));

    let out = scratch("rewrite-out").join("out");
    fractal::write(&dir, &out, &plan).unwrap();
    let compiled = fs::read_to_string(out.join("app/models/admin/report.rb")).unwrap();
    assert_eq!(compiled, REPORT.replace("\"original\" + NAME", "NAME.dup.prepend(\"rewritten\")"));
    assert_eq!(compiled.lines().count(), REPORT.lines().count());
    assert!(!out.join("app/models/current.rb").exists());
    assert_eq!(files_in(&out), [MANIFEST, "app/models/admin/report.rb"]);
    assert_eq!(fs::read_to_string(dir.join("app/models/admin/report.rb")).unwrap(), REPORT);
    let manifest: serde_json::Value = serde_json::from_str(&fs::read_to_string(out.join(MANIFEST)).unwrap()).unwrap();
    assert_eq!(manifest["rewritten"][0], "app/models/admin/report.rb");
    assert_eq!(manifest["compiled"][0], "app/models/admin/report.rb");
    assert_eq!(manifest["passes"][0], "test-swap");
}

#[test]
fn edits_that_leave_the_file_overlap_move_lines_or_break_the_parse_are_refused() {
    let dir = app_dir("refusals", &base_files());
    let cases: Vec<(&str, Fixed)> = vec![
        ("outside the file", Fixed(Box::new(|u| vec![Edit { span: u.span(u.text.len(), u.text.len() + 1), text: String::new() }]))),
        (
            "overlaps another edit",
            Fixed(Box::new(|u| {
                let start = u.text.find("def outer_label").unwrap();
                vec![Edit { span: u.span(start, start + 3), text: "def".into() }, Edit { span: u.span(start + 2, start + 4), text: "f ".into() }]
            })),
        ),
        ("changes the number of lines", Fixed(Box::new(|u| vec![at(u, "\"original\"", "\"a\"\n    \"b\"")]))),
        ("parse error", Fixed(Box::new(|u| vec![at(u, "\"original\"", "\"unterminated")]))),
    ];
    for (reason, pass) in &cases {
        let plan = build_plan(&dir, &[OUTER_RB], &[pass]).unwrap();
        assert!(plan.rewritten.is_empty(), "{reason}");
        assert_eq!(plan.report.declined.len(), 1, "{reason}: {:?}", plan.report.declined);
        assert_eq!(plan.report.declined[0].pass, "test-fixed");
        assert!(plan.report.declined[0].reason.contains(reason), "{reason}: {:?}", plan.report.declined);
    }
    // A refused proposal does not take a later pass's edits with it.
    let swap = Swap::new("\"original\"", "\"rewritten\"");
    let plan = build_plan(&dir, &[OUTER_RB], &[&cases[0].1, &swap]).unwrap();
    assert_eq!(plan.report.rewrites.len(), 3);
    assert_eq!(plan.report.declined.len(), 1);
}

#[test]
fn a_file_with_an_ingest_gap_keeps_the_apps_code() {
    let dir = app_dir("untrusted", &base_files());
    let mut app = ingest_app(&dir).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let swap = Swap::new("\"original\"", "\"rewritten\"");
    let plan = fractal::plan(&app, &dir, &[OUTER_RB.into()], &[OUTER_RB.into()], &[&swap]).unwrap();
    assert!(plan.rewritten.is_empty());
    assert!(plan.report.skipped[0].reason.contains("ingest could not read"), "{:?}", plan.report.skipped);
}

#[test]
fn definitions_on_other_objects_leave_their_file_untrusted_under_survey() {
    let mut files = base_files();
    files.push(("app/models/foreign.rb", FOREIGN));
    let dir = app_dir("foreign", &files);
    roundhouse::ingest::survey::activate();
    let app = ingest_app(&dir);
    let gaps = roundhouse::ingest::survey::drain();
    let mut app = app.unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let untrusted: Vec<String> = gaps
        .iter()
        .filter_map(|g| match g {
            roundhouse::ingest::IngestError::Unsupported { file, .. } => Some(file.clone()),
            _ => None,
        })
        .collect();
    assert!(!untrusted.is_empty());
    let swap = Swap::new("\"original\"", "\"rewritten\"");
    let plan = fractal::plan(&app, &dir, &["app/models/foreign.rb".into()], &untrusted, &[&swap]).unwrap();
    assert!(plan.rewritten.is_empty());
    assert_eq!(plan.report.skipped[0].file, "app/models/foreign.rb");
}

#[test]
fn a_rebuild_replaces_its_own_output_and_refuses_anything_else() {
    let dir = app_dir("rebuild", &base_files());
    let out = scratch("rebuild-out").join("out");
    let swap = Swap::new("\"original\"", "\"rewritten\"");
    fractal::write(&dir, &out, &build_plan(&dir, &[OUTER_RB], &[&swap]).unwrap()).unwrap();
    assert!(fs::read_to_string(out.join("app/models/outer.rb")).unwrap().contains("rewritten"));
    // Deselected: the earlier build is replaced, so the rewrite is gone.
    fractal::write(&dir, &out, &build_plan(&dir, &[], &[]).unwrap()).unwrap();
    assert!(!out.join("app/models/outer.rb").exists());
    assert_eq!(fs::read_to_string(dir.join("app/models/outer.rb")).unwrap(), NESTED);

    // Not an earlier build.
    let foreign = scratch("rebuild-foreign");
    fs::write(foreign.join("keep.rb"), "").unwrap();
    let err = fractal::write(&dir, &foreign, &build_plan(&dir, &[], &[]).unwrap()).err().unwrap();
    assert!(err.contains("is not an earlier fractal build"), "{err}");
    assert!(foreign.join("keep.rb").exists());
    // The app itself, or a directory holding it.
    for target in [dir.clone(), dir.parent().unwrap().to_path_buf()] {
        let err = fractal::write(&dir, &target, &build_plan(&dir, &[], &[]).unwrap()).err().unwrap();
        assert!(err.contains("is the app or contains it"), "{err}");
    }
    assert_eq!(fs::read_to_string(dir.join("app/models/current.rb")).unwrap(), CURRENT);
}

#[test]
fn repeated_emission_inside_the_app_does_not_copy_the_app() {
    let dir = app_dir("nested-out", &base_files());
    let out = dir.join("out/fractal");
    fractal::write(&dir, &out, &build_plan(&dir, &[], &[]).unwrap()).unwrap();
    // A second build sees the first one inside the app and leaves it out.
    fractal::write(&dir, &out, &build_plan(&dir, &[], &[]).unwrap()).unwrap();
    assert!(!out.join("out").exists());
    assert_eq!(files_in(&out), [MANIFEST]);
}

#[test]
fn the_cli_builds_reports_and_rejects_fractal_flags_elsewhere() {
    let dir = app_dir("cli", &base_files());
    let base = scratch("cli-out");
    let out = base.join("out");
    let report = base.join("report.json");
    let run = |args: &[&str]| Command::new(env!("CARGO_BIN_EXE_roundhouse")).args(args).output().unwrap();

    let ok = run(&["--target", "fractal", "--file", CURRENT_RB, "--report", report.to_str().unwrap(), dir.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&ok.stderr);
    assert!(ok.status.success(), "{stderr}");
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&report).unwrap()).unwrap();
    assert_eq!(json["compiled"][0], CURRENT_RB);
    assert_eq!(json["methods"].as_array().unwrap().len(), 6);
    assert_eq!(files_in(&out), [MANIFEST]);
    assert_eq!(fs::read_to_string(dir.join("app/models/current.rb")).unwrap(), CURRENT);

    let misplaced = run(&["--target", "ruby", "--file", CURRENT_RB, dir.to_str().unwrap()]);
    assert!(!misplaced.status.success());
    let gone = run(&["--target", "fractal", "--class", "Current", dir.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(!gone.status.success());
    let missing = run(&["--target", "fractal", "--file", "app/models/missing.rb", dir.to_str().unwrap(), "-o", out.to_str().unwrap()]);
    assert!(!missing.status.success());
}

// Scope: what a build ingests.

/// A pass that only declares a scope, for the driver's decision.
struct Scoped(fn() -> Scope);

impl Pass for Scoped {
    fn name(&self) -> &'static str {
        "test-scoped"
    }

    fn scope(&self) -> Scope {
        (self.0)()
    }

    fn run(&self, _context: &Context<'_>, _unit: &Unit<'_>) -> Proposal {
        Proposal::default()
    }
}

/// A file full ingest stops at: a migration verb the schema fold does
/// not take.
const UNREAD: &str = r#"class Unread < ActiveRecord::Migration[7.0]
  def change
    change_table :accounts do |t|
      t.string :slug
    end
  end
end
"#;

fn scoped_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("app/models/base_widget.rb", "class BaseWidget\n  def kind\n    \"base\"\n  end\nend\n"),
        ("app/models/concerns/labelled.rb", "module Labelled\n  def tag\n    \"tag\"\n  end\nend\n"),
        ("app/models/widget.rb", "class Widget < BaseWidget\n  include Labelled\n\n  def label\n    \"original\"\n  end\nend\n"),
        ("app/models/widget/reopened.rb", "class Widget\n  def other\n    \"original\"\n  end\nend\n"),
        ("app/models/unrelated.rb", "class Unrelated\n  def label\n    \"original\"\n  end\nend\n"),
        ("db/migrate/20240101000000_unread.rb", UNREAD),
        ("config/application.rb", "module App\n  class Application < Rails::Application\n  end\nend\n"),
    ]
}

fn decide(dir: &Path, files: &[&str], passes: &[&dyn Pass], whole_app: bool) -> Result<fractal::Ingest, String> {
    let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
    fractal::scope::decide(&dir.canonicalize().unwrap(), &files, passes, whole_app)
}

const WIDGET_RB: &str = "app/models/widget.rb";

/// The scope's files, relative to the app root.
fn scope_files(ingest: &fractal::Ingest, dir: &Path) -> Vec<String> {
    let fractal::Ingest::Files { files, .. } = ingest else { panic!("expected a scoped ingest") };
    let root = dir.canonicalize().unwrap();
    files.iter().map(|f| f.strip_prefix(&root).unwrap().to_string_lossy().into_owned()).collect()
}

#[test]
fn a_build_ingests_only_the_compiled_files_and_the_apps_shape_by_default() {
    let dir = app_dir("scope-default", &scoped_files());
    let files_only = Scoped(|| Scope::Files(Vec::new()));
    let ingest = decide(&dir, &[WIDGET_RB], &[&files_only], false).unwrap();
    assert_eq!(scope_files(&ingest, &dir), ["app/models/widget.rb", "config/application.rb"]);
    let fractal::Ingest::Files { indexed, .. } = ingest else { unreachable!() };
    assert_eq!(indexed, 0, "no reach, so nothing to index");
    // No pass registered is the same: files are the unit.
    assert_eq!(scope_files(&decide(&dir, &[WIDGET_RB], &[], false).unwrap(), &dir).len(), 2);
}

#[test]
fn ancestors_bring_in_the_files_defining_superclasses_and_mixins() {
    let dir = app_dir("scope-ancestors", &scoped_files());
    let pass = Scoped(|| Scope::Files(vec![fractal::Reach::Ancestors]));
    let files = scope_files(&decide(&dir, &[WIDGET_RB], &[&pass], false).unwrap(), &dir);
    assert!(files.contains(&"app/models/base_widget.rb".to_string()), "{files:?}");
    assert!(files.contains(&"app/models/concerns/labelled.rb".to_string()), "{files:?}");
    assert!(!files.iter().any(|f| f.contains("unrelated") || f.contains("migrate")), "{files:?}");
}

#[test]
fn schema_signatures_and_custom_reaches_add_their_files() {
    let mut files = scoped_files();
    files.push(("db/schema.rb", SCHEMA));
    files.push(("sig/widget.rbs", "class Widget\n  def label: () -> String\nend\n"));
    let dir = app_dir("scope-reaches", &files);
    let pass = Scoped(|| {
        Scope::Files(vec![
            fractal::Reach::Schema,
            fractal::Reach::Signatures,
            fractal::Reach::Custom(|context, files| {
                files.extend(context.index.files["Unrelated"].iter().map(PathBuf::from));
            }),
        ])
    });
    let scope = scope_files(&decide(&dir, &[WIDGET_RB], &[&pass], false).unwrap(), &dir);
    for expected in ["db/schema.rb", "sig/widget.rbs", "app/models/unrelated.rb"] {
        assert!(scope.contains(&expected.to_string()), "{expected} missing from {scope:?}");
    }
}

#[test]
fn a_pass_that_needs_the_whole_app_or_whole_app_reads_everything() {
    let dir = app_dir("scope-whole", &scoped_files());
    let whole = Scoped(|| Scope::WholeApp);
    let classes_only = Scoped(|| Scope::Files(Vec::new()));
    match decide(&dir, &[WIDGET_RB], &[&classes_only, &whole], false).unwrap() {
        fractal::Ingest::WholeApp { reason } => assert_eq!(reason, "pass `test-scoped` needs the whole app"),
        fractal::Ingest::Files { .. } => panic!("a whole-app pass must widen the build"),
    }
    match decide(&dir, &[WIDGET_RB], &[&classes_only], true).unwrap() {
        fractal::Ingest::WholeApp { reason } => assert_eq!(reason, "--whole-app"),
        fractal::Ingest::Files { .. } => panic!("--whole-app must read everything"),
    }
}

#[test]
fn a_file_that_does_not_exist_is_an_error_before_anything_is_ingested() {
    let dir = app_dir("scope-missing", &scoped_files());
    let err = decide(&dir, &["app/models/missing.rb"], &[], false).err().unwrap();
    assert!(err.contains("--file app/models/missing.rb: no such file in the app"), "{err}");
    let root = dir.canonicalize().unwrap();
    let err = fractal::scope::normalize_files(&root, &["../outside.rb".to_string()]).err().unwrap();
    assert!(err.contains("no such file") || err.contains("outside the app"), "{err}");
    let abs = root.join(WIDGET_RB).to_string_lossy().into_owned();
    assert_eq!(fractal::scope::normalize_files(&root, &[abs, WIDGET_RB.to_string()]).unwrap(), [WIDGET_RB]);
}

/// Ingest through the scope sees only its files, so a file whole-app
/// ingest stops at is never read, and the compiled files plan exactly
/// as it does in a whole-app build.
#[test]
fn a_scoped_ingest_reads_only_its_files_and_plans_them_the_same() {
    let dir = app_dir("scope-ingest", &scoped_files());
    assert!(ingest_app(&dir).is_err(), "the migration stops a whole-app ingest");
    let root = dir.canonicalize().unwrap();
    let compiled = ["app/models/widget.rb", "app/models/widget/reopened.rb"];
    let fractal::Ingest::Files { files, .. } = decide(&dir, &compiled, &[], false).unwrap() else { unreachable!() };
    let fs = roundhouse::vfs::FsVfs::new();
    let vfs = roundhouse::vfs::ScopedVfs::new(&fs, files);
    let mut app = roundhouse::ingest::app::ingest_app_with_vfs(&vfs, &root).unwrap();
    let read: Vec<&str> = app.sources.iter().map(|s| s.path.as_str()).filter(|p| p.ends_with(".rb")).collect();
    assert!(read.iter().all(|p| !p.contains("unrelated") && !p.contains("migrate") && !p.contains("base_widget")), "{read:?}");
    Analyzer::new(&app).analyze(&mut app);
    let swap = Swap::new("original", "rewritten");
    let compiled: Vec<String> = compiled.iter().map(|f| f.to_string()).collect();
    let plan = fractal::plan(&app, &root, &compiled, &[], &[&swap]).unwrap();
    assert_eq!(keys(&plan), ["Widget#label", "Widget#other"]);
    assert_eq!(plan.report.rewritten, compiled);
}

/// A component: an in-repository gem with its own `app/`, which ingest
/// takes for an app root because its `lib/` declares a `Rails::Engine`.
#[test]
fn a_component_file_brings_its_engine_declaration_into_scope() {
    let mut files = scoped_files();
    files.retain(|(p, _)| !p.starts_with("db/"));
    files.push(("Gemfile.lock", "PATH\n  remote: components/billing\n  specs:\n    billing (0.1.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  billing!\n"));
    files.push(("components/billing/billing.gemspec", "Gem::Specification.new { |s| s.name = \"billing\" }\n"));
    files.push(("components/billing/lib/billing/engine.rb", "module Billing\n  class Engine < Rails::Engine\n  end\nend\n"));
    files.push(("components/billing/app/models/billing/invoice.rb", "module Billing\n  class Invoice\n    def label\n      \"original\"\n    end\n  end\nend\n"));
    let dir = app_dir("scope-component", &files);
    let invoice = "components/billing/app/models/billing/invoice.rb";
    let ingest = decide(&dir, &[invoice], &[], false).unwrap();
    let scope = scope_files(&ingest, &dir);
    assert!(scope.contains(&"components/billing/lib/billing/engine.rb".to_string()), "{scope:?}");
    let fractal::Ingest::Files { files, .. } = ingest else { unreachable!() };
    let root = dir.canonicalize().unwrap();
    let fs = roundhouse::vfs::FsVfs::new();
    let mut app = roundhouse::ingest::app::ingest_app_with_vfs(&roundhouse::vfs::ScopedVfs::new(&fs, files), &root).unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let plan = fractal::plan(&app, &root, &[invoice.to_string()], &[], &[]).unwrap();
    assert_eq!(keys(&plan), ["Billing::Invoice#label"]);
}

#[test]
fn invalid_utf8_edit_and_decline_offsets_report_without_panicking() {
    struct InvalidOffsets;
    impl Pass for InvalidOffsets {
        fn name(&self) -> &'static str { "test-invalid-offsets" }
        fn scope(&self) -> Scope { Scope::Files(Vec::new()) }
        fn run(&self, _context: &Context<'_>, unit: &Unit<'_>) -> Proposal {
            let inside = unit.text.find('é').unwrap() + 1;
            Proposal {
                edits: vec![Edit { span: unit.span(inside, inside + 1), text: "x".into() }],
                declined: vec![
                    (unit.span(inside, inside), "inside a character".into()),
                    (unit.span(unit.text.len() + 10, unit.text.len() + 10), "past the file".into()),
                ],
            }
        }
    }
    let source = "# café\nclass Widget\n  def value = 1\nend\n";
    let dir = app_dir("utf8-offsets", &[("app/models/widget.rb", source)]);
    let plan = build_plan(&dir, &["app/models/widget.rb"], &[&InvalidOffsets]).unwrap();
    assert!(plan.rewritten.is_empty());
    assert_eq!(
        plan.report.declined.iter().map(|d| d.at.as_str()).collect::<Vec<_>>(),
        ["app/models/widget.rb:1:6", "app/models/widget.rb:5:1", "app/models/widget.rb:1:6"]
    );
    assert_eq!(fs::read_to_string(dir.join("app/models/widget.rb")).unwrap(), source);
}

#[test]
fn an_insertion_before_a_replacement_at_the_same_start_keeps_both_edits() {
    let source = "class Widget\n  def value = \"original\"\nend\n";
    let dir = app_dir("same-start", &[("app/models/widget.rb", source)]);
    let pass = Fixed(Box::new(|unit| {
        let replacement = at(unit, "\"original\"", "\"replaced\"");
        let insertion = Edit {
            span: unit.span(replacement.span.start as usize, replacement.span.start as usize),
            text: "\"prefix\" + ".into(),
        };
        vec![insertion, replacement]
    }));
    let plan = build_plan(&dir, &["app/models/widget.rb"], &[&pass]).unwrap();
    assert!(plan.report.declined.is_empty(), "{:?}", plan.report.declined);
    let output = Command::new("ruby").arg("-e").arg(format!("{}\nputs Widget.new.value", plan.rewritten[0].1)).output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "prefixreplaced\n");
}

#[test]
fn ancestor_scope_keeps_nested_namespace_directories() {
    let dir = app_dir("namespace-scope", &[
        ("app/models/widget.rb", "class Widget < Public::BaseWidget\nend\n"),
        ("app/models/public/base_widget.rb", "module Public\n  class BaseWidget\n  end\nend\n"),
    ]);
    let pass = Scoped(|| Scope::Files(vec![fractal::Reach::Ancestors]));
    let scope = scope_files(&decide(&dir, &["app/models/widget.rb"], &[&pass], false).unwrap(), &dir);
    assert!(scope.contains(&"app/models/public/base_widget.rb".to_string()), "{scope:?}");
}

#[test]
fn a_pass_that_keeps_the_same_text_emits_no_app_file() {
    let dir = app_dir("no-op", &base_files());
    let pass = Swap::new("original", "original");
    let plan = build_plan(&dir, &[CURRENT_RB], &[&pass]).unwrap();
    assert!(plan.rewritten.is_empty());
    assert!(plan.report.rewritten.is_empty());
}

/// Plan `compiled` the way the CLI does by default: ingest only what the
/// scope decides, recording gaps (`--survey`) so their files are skipped
/// instead of stopping the build.
fn scoped_plan(dir: &Path, compiled: &[&str], passes: &[&dyn Pass]) -> Plan {
    let root = dir.canonicalize().unwrap();
    let fractal::Ingest::Files { files, .. } = decide(dir, compiled, passes, false).unwrap() else { panic!("expected a scoped ingest") };
    let filesystem = roundhouse::vfs::FsVfs::new();
    let vfs = roundhouse::vfs::ScopedVfs::new(&filesystem, files);
    roundhouse::ingest::survey::activate();
    let app = roundhouse::ingest::app::ingest_app_with_vfs(&vfs, &root);
    let gaps = roundhouse::ingest::survey::drain();
    let mut app = app.unwrap();
    Analyzer::new(&app).analyze(&mut app);
    let untrusted: Vec<String> = gaps
        .iter()
        .filter_map(|gap| match gap {
            roundhouse::ingest::IngestError::Unsupported { file, .. } | roundhouse::ingest::IngestError::Parse { file, .. } => Some(file.clone()),
            _ => None,
        })
        .collect();
    let compiled: Vec<String> = compiled.iter().map(|file| file.to_string()).collect();
    fractal::plan(&app, &root, &compiled, &untrusted, passes).unwrap()
}

#[test]
fn a_class_inside_a_singleton_class_body_holds_instance_methods() {
    let source = "class Outer\n  class << self\n    def built; end\n\n    class Inner\n      def title; end\n    end\n  end\n\n  class << OTHER\n    class ::Plain\n      def label; end\n    end\n\n    def elsewhere; end\n  end\nend\n";
    let dir = app_dir("singleton-nesting", &[("app/models/outer.rb", source)]);
    let plan = scoped_plan(&dir, &["app/models/outer.rb"], &[]);
    assert_eq!(keys(&plan), ["OTHER.elsewhere", "Outer.built", "Outer::Inner#title", "Plain#label"]);
}

#[test]
fn a_relative_namespace_defined_in_another_file_gets_its_full_name() {
    let compiled = "module Top\n  class Admin::Report\n    def title; end\n  end\nend\n";
    let dir = app_dir(
        "relative-namespace",
        &[("app/models/top/admin/report.rb", compiled), ("app/models/top/admin.rb", "module Top\n  module Admin\n  end\nend\n")],
    );
    // A scoped build ingests only the compiled file, which does not say
    // where `Admin` is; the app's sources do.
    let plan = scoped_plan(&dir, &["app/models/top/admin/report.rb"], &[]);
    assert_eq!(keys(&plan), ["Top::Admin::Report#title"]);

    // A namespace no app file defines (a gem's, say) resolves to the
    // top-level constant, as Ruby's lexical lookup falls back to it.
    let dir = app_dir("relative-namespace-gem", &[("app/models/top/report.rb", compiled)]);
    let plan = scoped_plan(&dir, &["app/models/top/report.rb"], &[]);
    assert_eq!(keys(&plan), ["Admin::Report#title"]);
}

#[test]
fn a_report_inside_the_overlay_is_refused_before_anything_is_written() {
    let dir = app_dir("report-in-overlay", &base_files());
    let base = scratch("report-in-overlay-out");
    let run = |arguments: &[&str]| Command::new(env!("CARGO_BIN_EXE_fractal")).args(arguments).output().unwrap();

    // Next to an earlier build: the rebuild would delete the report.
    let overlay = base.join("overlay");
    let built = run(&["emit", "--file", CURRENT_RB, dir.to_str().unwrap(), "-o", overlay.to_str().unwrap()]);
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let inside = overlay.join("report.json");
    let refused = run(&["emit", "--file", CURRENT_RB, "--report", inside.to_str().unwrap(), dir.to_str().unwrap(), "-o", overlay.to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&refused.stderr).contains("is inside the overlay"), "{}", String::from_utf8_lossy(&refused.stderr));
    assert!(!inside.exists());

    // In an unrelated directory: a report named like the manifest must not
    // make it look like an earlier build, which emit would then replace.
    let precious = base.join("precious");
    fs::create_dir_all(&precious).unwrap();
    fs::write(precious.join("keep.txt"), "keep").unwrap();
    let marker = precious.join(MANIFEST);
    let refused = run(&["emit", "--file", CURRENT_RB, "--report", marker.to_str().unwrap(), dir.to_str().unwrap(), "-o", precious.to_str().unwrap()]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(!marker.exists());
    assert_eq!(fs::read_to_string(precious.join("keep.txt")).unwrap(), "keep");

    // Outside the overlay the report is written after the overlay.
    let report = base.join("report.json");
    let emitted = run(&["emit", "--file", CURRENT_RB, "--report", report.to_str().unwrap(), dir.to_str().unwrap(), "-o", overlay.to_str().unwrap()]);
    assert!(emitted.status.success(), "{}", String::from_utf8_lossy(&emitted.stderr));
    assert!(report.exists());
}

#[test]
fn a_file_that_is_not_ruby_is_refused() {
    let mut files = base_files();
    files.push(("config/settings.yml", "key: value\n"));
    let dir = app_dir("not-ruby", &files);
    let error = fractal::scope::normalize_files(&dir.canonicalize().unwrap(), &["config/settings.yml".to_string()]).err().unwrap();
    assert!(error.contains("--file config/settings.yml: not a Ruby source (.rb)"), "{error}");
}

#[test]
fn a_multi_line_rewrite_renders_on_one_line() {
    let dir = app_dir("one-line", &base_files());
    let pass = Fixed(Box::new(|unit| vec![at(unit, "def outer_label\n    \"original\"", "def outer_label\n    \"rewritten\"")]));
    let plan = build_plan(&dir, &[OUTER_RB], &[&pass]).unwrap();
    let rendered = plan.report.render();
    let line = rendered.lines().find(|line| line.contains("rewrote")).unwrap_or_else(|| panic!("{rendered}"));
    assert!(line.ends_with(r#"def outer_label\n    "original" -> def outer_label\n    "rewritten""#), "{line}");
}
