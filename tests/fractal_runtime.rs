//! The fractal runtime (`src/emit/fractal/fractal.rb`) against real
//! Ruby: build an app with a test pass, then load the generated
//! initializer and the app's classes the way Rails would
//! (`tests/support/fractal_runtime_test.rb`), and check which variants
//! installed, which declined and why, and that an installed one keeps
//! the original's visibility, nesting, `super`, file and line.
//!
//! Needs `ruby` (with minitest, a bundled gem) on PATH. The Active
//! Record and Current attributes cases run when `activerecord` is
//! installed, as CI's unit job does, and skip otherwise.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::fractal::{self, Cx, Edit, Guard, Pass, Proposal, Region};
use roundhouse::ingest::ingest_app;
use roundhouse::span::Span;

const FILES: &[(&str, &str)] = &[
    ("app/models/plain.rb", "class Plain\n  def label\n    \"original\"\n  end\nend\n"),
    (
        "app/models/hidden.rb",
        "class Hidden\n  def reveal\n    secret\n  end\n\n  private def secret\n    \"original\"\n  end\nend\n",
    ),
    (
        "app/models/admin/report.rb",
        "module Admin\n  NAME = \"-report\"\n\n  class Report\n    def title\n      \"original\" + NAME\n    end\n  end\nend\n",
    ),
    ("app/models/frozen.rb", "# frozen_string_literal: true\n\nclass Frozen\n  def text\n    \"original\"\n  end\nend\n"),
    (
        "app/models/sing.rb",
        "class Sing\n  def self.make\n    \"original\"\n  end\n\n  class << self\n    def other\n      \"original\"\n    end\n  end\nend\n",
    ),
    (
        "app/models/patched.rb",
        "class Patched\n  def label\n    \"original\"\n  end\n\n  prepend(Module.new do\n    def label\n      super + \"!\"\n    end\n  end)\nend\n",
    ),
    ("app/models/edited.rb", "class Edited\n  def label\n    \"original\"\n  end\nend\n"),
    ("app/models/base.rb", "class Base\n  def label\n    \"base\"\n  end\nend\n"),
    ("app/models/child.rb", "class Child < Base\n  def label\n    super + \"-original\"\n  end\nend\n"),
    ("app/models/guarded.rb", "class Guarded\n  def label\n    \"original\"\n  end\nend\n"),
    ("app/models/guarded_ok.rb", "class GuardedOk\n  def label\n    \"original\"\n  end\nend\n"),
    (
        "app/models/hooked.rb",
        "class Hooked\n  def self.method_added(name)\n    (@added ||= []) << name\n  end\n\n  def label\n    \"original\"\n  end\nend\n",
    ),
    ("app/models/raiser.rb", "class Raiser\n  def boom\n    raise \"original\"\n  end\nend\n"),
    ("app/models/eager.rb", "class Eager\n  def label\n    \"original\"\n  end\nend\n"),
    ("app/models/guarded_late.rb", "class GuardedLate\n  def label\n    \"original\"\n  end\nend\n"),
    (
        "app/models/record.rb",
        "class Record < ActiveRecord::Base\n  self.abstract_class = true\n\n  def label\n    \"original\"\n  end\n\n  def self.make\n    \"original\"\n  end\nend\n",
    ),
    ("app/models/current.rb", "class Current < ActiveSupport::CurrentAttributes\n  def label\n    \"original\"\n  end\nend\n"),
    ("app/models/late.rb", "class Late\nend\n"),
    ("app/models/late/reopened.rb", "class Late\n  def label\n    \"original\"\n  end\nend\n"),
    ("app/models/counter.rb", "class Counter\n  def bump\n    @count = (@count || 0) + 1\n    \"original\"\n  end\nend\n"),
];

const CLASSES: &[&str] =
    &["Plain", "Hidden", "Admin::Report", "Frozen", "Sing", "Patched", "Edited", "Child", "Guarded", "GuardedOk", "Hooked", "Raiser", "Eager", "Counter", "Late", "GuardedLate", "Record", "Current"];

/// Swaps `original` for `variant` inside every selected method, and
/// gives three classes a guard: on `Plain#label` at its line, on
/// `Plain#label` at the wrong line, and on `Eager#label`, which loads
/// late. A test fixture, not an optimization.
struct Swap;

impl Pass for Swap {
    fn name(&self) -> &'static str {
        "test-swap"
    }

    fn run(&self, cx: &Cx<'_>, region: &Region<'_>) -> Proposal {
        let text = &cx.app.sources[region.file.0 as usize - 1].text;
        let (start, end) = (region.def_span.start as usize, region.def_span.end as usize);
        let edits = text[start..end]
            .match_indices("original")
            .map(|(at, _)| Edit {
                span: Span { file: region.file, start: (start + at) as u32, end: (start + at + 8) as u32 },
                text: "variant".into(),
            })
            .collect();
        let guard = |constant: &str, line| Guard::DefinedAt {
            constant: constant.into(),
            method: "label".into(),
            singleton: false,
            file: format!("app/models/{}.rb", constant.to_lowercase()),
            line,
        };
        let guards = match region.owner.as_str() {
            "GuardedOk" => vec![guard("Plain", 2)],
            "Guarded" => vec![guard("Plain", 3)],
            "GuardedLate" => vec![guard("Eager", 2)],
            _ => Vec::new(),
        };
        Proposal { edits, guards, declined: Vec::new(), replayable: true }
    }
}

fn build() -> PathBuf {
    let base = std::env::temp_dir().join(format!("roundhouse-fractal-runtime-{}", std::process::id()));
    let _ = fs::remove_dir_all(&base);
    let app_dir = base.join("app");
    for (path, content) in FILES {
        let path = app_dir.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }
    let mut app = ingest_app(&app_dir).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    let classes: Vec<String> = CLASSES.iter().map(|c| c.to_string()).collect();
    let plan = fractal::plan(&app, &app_dir, &classes, &[], &[&Swap]).expect("plan");
    // The one method not admitted is the prepended module's, written in
    // a block inside Patched's body.
    let refused: Vec<_> = plan.report.regions.iter().filter(|r| !r.admitted).map(|r| r.key.as_str()).collect();
    assert_eq!(refused, ["Patched#label@app/models/patched.rb:7"]);
    // Every other method with `original` in it.
    assert_eq!(plan.report.variants.len(), 20, "{:?}", plan.report.variants.iter().map(|v| &v.key).collect::<Vec<_>>());
    // The app with the overlay placed on its root, as a deploy would.
    let overlay = base.join("overlay");
    fractal::write(&overlay, &plan).expect("write");
    let out = base.join("out");
    assert!(Command::new("cp").arg("-a").arg(&app_dir).arg(&out).status().unwrap().success());
    assert!(Command::new("cp").arg("-a").arg(format!("{}/.", overlay.display())).arg(&out).status().unwrap().success());
    out
}

fn ruby(out: &PathBuf, mode: &str) {
    let root = env!("CARGO_MANIFEST_DIR");
    let output = Command::new("ruby")
        .arg(format!("{root}/tests/support/fractal_runtime_test.rb"))
        .env("FRACTAL_APP", out)
        .env("FRACTAL_MODE", mode)
        .env("FRACTAL_VERIFY", if mode == "verify" { "1" } else { "0" })
        .env("FRACTAL_STRICT", if mode == "strict" { "1" } else { "0" })
        .output()
        .expect("ruby must be on PATH");
    assert!(
        output.status.success(),
        "mode {mode}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn the_runtime_installs_variants_whose_guards_hold_and_keeps_the_app_otherwise() {
    let out = build();
    for mode in ["plain", "verify", "strict"] {
        // Each run edits a file in the tree, so each gets its own copy.
        let copy = out.with_file_name(format!("out-{mode}"));
        let _ = fs::remove_dir_all(&copy);
        let status = Command::new("cp").arg("-a").arg(&out).arg(&copy).status().unwrap();
        assert!(status.success());
        ruby(&copy, mode);
    }
}
