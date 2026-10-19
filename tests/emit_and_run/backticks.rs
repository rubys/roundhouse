//! Backticks on the CRuby overlay; the native twin is
//! `spinel_toolchain/backticks.rs`.

use super::backticks_contract as contract;

/// A backtick literal and `%x{}` run the command, as CRuby runs them.
#[test]
fn backticks_run_the_command() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "stderr:\n{}", run.stderr);
}

/// Strict targets have no shell-out. The shared emit gate records
/// `shell_command` so `check` is not clean, and the ruby family does not.
#[test]
fn strict_targets_ledger_the_shell_out() {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use roundhouse::diagnostic::{DiagnosticKind, Severity};
    use roundhouse::project::{target_files, BuildTarget};

    let files = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table :articles do |t|\n    t.string :title\n    t.integer :author_id\n  end\n  create_table :comments do |t|\n    t.integer :article_id\n    t.string :host\n  end\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :articles, only: :index\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/article.rb", "class Article < ApplicationRecord\n  belongs_to :author, default: -> { `whoami`.strip }\n  has_many :comments, -> { where(host: `hostname`.strip) }\n  def host\n    `hostname`.strip\n  end\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files.into_iter().map(|(p, c)| (p.into(), c.into())).collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let shell = |diags: &[roundhouse::diagnostic::Diagnostic]| {
        diags.iter().any(|d| {
            d.severity == Severity::Error
                && matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "shell_command")
        })
    };
    for target in [
        BuildTarget::Crystal,
        BuildTarget::Elixir,
        BuildTarget::Go,
        BuildTarget::Kotlin,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Swift,
        BuildTarget::CSharp,
        BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
    ] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| target_files(&app, std::path::Path::new("."), target));
        files.expect("target files");
        let n = diags.iter().filter(|d| {
            d.severity == Severity::Error
                && matches!(&d.kind, DiagnosticKind::Unsupported { construct, .. } if construct.as_str() == "shell_command")
        }).count();
        // Method body, belongs_to default, and has_many scope.
        assert_eq!(n, 3, "{target:?} ledgered {n} shell-outs, not the three sites: {diags:?}");
    }
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        let (files, diags) = roundhouse::emit::diagnostics::scope(|| target_files(&app, std::path::Path::new("."), target));
        files.expect("target files");
        assert!(!shell(&diags), "{target:?} ledgered a shell-out it can run: {diags:?}");
    }
}
