//! A plain Ruby class in the app (`app/models/probe.rb` holding
//! `class Probe`) is an `app.library_classes` entry. The ruby family
//! and TypeScript emit it. The other emitters did not, and said
//! nothing: the test calling `Probe.run` was emitted against a class
//! that was not, and the transpile reported zero errors. Each of those
//! targets now reports the class as unsupported.
//!
//! The production gate denylists emitters that already carry plain
//! library classes (fail-closed). This suite partitions
//! `BuildTarget::TRANSPILE` against that contract so a new target
//! cannot stay silent without updating the denylist.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

fn app() -> roundhouse::App {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/widget.rb"),
        b"class Widget < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/probe.rb"),
        b"class Probe\n  def self.run\n    1 + 2\n  end\nend\n".to_vec(),
    );
    // Explicit Object is the same terminal as an omitted superclass.
    tree.insert(
        PathBuf::from("app/models/probe_object.rb"),
        b"class ProbeObject < Object\n  def self.run\n    1 + 2\n  end\nend\n".to_vec(),
    );
    // Framework-based classes are not plain Ruby and stay unreported.
    tree.insert(
        PathBuf::from("app/jobs/application_job.rb"),
        b"class ApplicationJob < ActiveJob::Base\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/mailers/application_mailer.rb"),
        b"class ApplicationMailer < ActionMailer::Base\n  default from: \"a@b.c\"\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/models/concerns/greeting.rb"),
        b"module Greeting\n  def greet\n    \"hi\"\n  end\nend\n".to_vec(),
    );
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let mut analyzer = Analyzer::new(&app);
    analyzer.analyze(&mut app);
    app
}

fn reported(app: &roundhouse::App, target: BuildTarget) -> Vec<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/tiny-blog");
    let (_, diags) = roundhouse::emit::diagnostics::scope(|| target_files(app, &root, target));
    diags
        .into_iter()
        .filter(|d| d.message.contains("plain Ruby class"))
        .map(|d| d.message)
        .collect()
}

/// Mirrors `target_emits_app_library_classes` in `src/project.rs`.
/// Keep in lockstep: the partition test fails if a TRANSPILE target is
/// added without updating both sides.
fn emits_plain_library_classes(target: BuildTarget) -> bool {
    matches!(
        target,
        BuildTarget::Ruby
            | BuildTarget::Jruby
            | BuildTarget::Spinel
            | BuildTarget::Typescript
            | BuildTarget::TypescriptWorker
            | BuildTarget::Futamura
        // Roda omitted — same as production: spike does not emit POROs.
    )
}

#[test]
fn plain_class_reporting_partitions_transpile_targets() {
    let app = app();
    for &target in BuildTarget::TRANSPILE {
        let msgs = reported(&app, target);
        if emits_plain_library_classes(target) {
            assert_eq!(
                msgs,
                Vec::<String>::new(),
                "{} should stay quiet: {msgs:?}",
                target.as_str()
            );
        } else {
            assert_eq!(msgs.len(), 2, "{}: {msgs:?}", target.as_str());
            assert!(
                msgs.iter().any(|m| m.contains("class `Probe`")),
                "{}: {msgs:?}",
                target.as_str()
            );
            assert!(
                msgs.iter().any(|m| m.contains("class `ProbeObject`")),
                "{}: {msgs:?}",
                target.as_str()
            );
            for m in &msgs {
                assert!(
                    m.contains(&format!("({})", target.as_str())),
                    "{}",
                    m
                );
            }
        }
    }
}
