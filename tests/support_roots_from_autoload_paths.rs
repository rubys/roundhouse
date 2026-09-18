//! Which directories hold an app's plain-Ruby classes is the app's
//! choice, not a list roundhouse can keep.
//!
//! Rails autoloads every `app/*` subdirectory; ingest walked a fixed
//! set of names (`app/services`, `app/workers`, `app/policies`, …). An
//! app whose use cases live in `app/interactors/` registered none of
//! them, so every call into that layer reported `send_dispatch_failed`
//! against a constant the analyzer had never seen (#86).
//!
//! The roots are discovered instead, and where discovery is wrong for a
//! tree the app says so itself — `config.autoload_lib(ignore:)` is
//! honored, with no heuristic for what merely looks like dev tooling.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "reports", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const CONTROLLER: &str = r#"class ReportsController < ApplicationController
  def show
    @name = Interactors::BuildReport.call
  end
end
"#;

const INTERACTOR: &str = r#"module Interactors
  class BuildReport
    def self.call
      "report"
    end
  end
end
"#;

fn diagnostics(files: &[(&str, &str)]) -> Vec<String> {
    let base: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/report.rb", "class Report < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/reports_controller.rb", CONTROLLER),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#show\"\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = base
        .iter()
        .chain(files.iter())
        .map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .collect()
}

fn dispatch_failed_on_the_interactor(diagnostics: &[String]) -> bool {
    diagnostics
        .iter()
        .any(|d| d.contains("send_dispatch_failed") && d.contains("BuildReport"))
}

#[test]
fn a_layer_the_app_named_itself_is_reachable_from_a_controller() {
    let diags = diagnostics(&[("app/interactors/build_report.rb", INTERACTOR)]);
    assert!(
        !dispatch_failed_on_the_interactor(&diags),
        "`app/interactors/` is autoloaded by Rails and has to register; diagnostics = {diags:?}"
    );
}

#[test]
fn an_ignored_lib_directory_stays_invisible() {
    // `autoload_lib(ignore: %w[custom_cops])` takes the directory off
    // the autoload paths; its classes are not app code, and a call into
    // one still reports.
    let application_rb = r#"module Blog
  class Application < Rails::Application
    config.autoload_lib(ignore: %w[custom_cops])
  end
end
"#;
    let diags = diagnostics(&[
        ("config/application.rb", application_rb),
        ("lib/custom_cops/build_report.rb", INTERACTOR),
    ]);
    assert!(
        dispatch_failed_on_the_interactor(&diags),
        "an ignored directory must stay off the walk; diagnostics = {diags:?}"
    );

    // …and the same file under a directory the app did not ignore is
    // ordinary app code again, so the ignore is doing the work rather
    // than the path being unreachable.
    let diags = diagnostics(&[
        ("config/application.rb", application_rb),
        ("lib/interactors/build_report.rb", INTERACTOR),
    ]);
    assert!(
        !dispatch_failed_on_the_interactor(&diags),
        "only the ignored directory is skipped; diagnostics = {diags:?}"
    );
}

#[test]
fn a_root_the_app_adds_to_the_eager_load_paths_is_walked() {
    let application_rb = r#"module Blog
  class Application < Rails::Application
    config.eager_load_paths << Rails.root.join("extra_domain")
  end
end
"#;
    let diags = diagnostics(&[
        ("config/application.rb", application_rb),
        ("extra_domain/build_report.rb", INTERACTOR),
    ]);
    assert!(
        !dispatch_failed_on_the_interactor(&diags),
        "a declared eager-load root is app code; diagnostics = {diags:?}"
    );
}

#[test]
fn the_directories_with_their_own_pass_are_not_walked_twice() {
    // `app/models` is ingested as models; re-walking it as library
    // classes would register the same constant twice.
    let diags = diagnostics(&[("app/interactors/build_report.rb", INTERACTOR)]);
    assert!(
        !diags.iter().any(|d| d.contains("duplicate")),
        "diagnostics = {diags:?}"
    );
}
