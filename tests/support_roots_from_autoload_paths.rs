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

fn diagnostics(files: &[(&str, &str)]) -> (roundhouse::App, Vec<roundhouse::diagnostic::Diagnostic>) {
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
    let mut diags = roundhouse::session::analyze_and_lower(&mut app);
    diags.extend(roundhouse::analyze::diagnose(&app));
    (app, diags)
}

fn interactor_error(app: &roundhouse::App, diags: &[roundhouse::diagnostic::Diagnostic]) -> bool {
    let Some((index, _)) = app.sources.iter().enumerate()
        .find(|(_, source)| source.path == "app/controllers/reports_controller.rb") else {
        return false;
    };
    let start = CONTROLLER.find("Interactors::BuildReport").unwrap();
    let end = start + "Interactors::BuildReport".len();
    diags.iter().any(|diagnostic| {
        diagnostic.severity == roundhouse::diagnostic::Severity::Error
            && diagnostic.span.file.0 as usize == index + 1
            && diagnostic.span.start as usize <= start
            && diagnostic.span.end as usize >= end
    })
}

#[test]
fn a_layer_the_app_named_itself_is_reachable_from_a_controller() {
    let (app, diags) = diagnostics(&[("app/interactors/build_report.rb", INTERACTOR)]);
    assert!(
        !interactor_error(&app, &diags),
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
    let (app, diags) = diagnostics(&[
        ("config/application.rb", application_rb),
        ("lib/custom_cops/build_report.rb", INTERACTOR),
    ]);
    assert!(
        interactor_error(&app, &diags),
        "an ignored directory must stay off the walk; diagnostics = {diags:?}"
    );

    // …and the same file under a directory the app did not ignore is
    // ordinary app code again, so the ignore is doing the work rather
    // than the path being unreachable.
    let (app, diags) = diagnostics(&[
        ("config/application.rb", application_rb),
        ("lib/interactors/build_report.rb", INTERACTOR),
    ]);
    assert!(
        !interactor_error(&app, &diags),
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
    let (app, diags) = diagnostics(&[
        ("config/application.rb", application_rb),
        ("extra_domain/build_report.rb", INTERACTOR),
    ]);
    assert!(
        !interactor_error(&app, &diags),
        "a declared eager-load root is app code; diagnostics = {diags:?}"
    );
}

#[test]
fn a_root_the_app_adds_to_the_autoload_once_paths_is_walked() {
    let application_rb = r##"module Blog
  class Application < Rails::Application
    config.autoload_lib(ignore: %w[autoload_once])
    config.autoload_once_paths << "#{root}/lib/autoload_once"
  end
end
"##;
    let (app, diags) = diagnostics(&[
        ("config/application.rb", application_rb),
        ("lib/autoload_once/interactors/build_report.rb", INTERACTOR),
    ]);
    assert!(
        !interactor_error(&app, &diags),
        "a declared autoload-once root is app code; diagnostics = {diags:?}"
    );
}

#[test]
fn a_root_written_under_the_interpolated_app_root_is_walked() {
    for root in ["root", "Rails.root", "config.root"] {
        let application_rb = format!(
            "module Blog\n  class Application < Rails::Application\n    config.autoload_paths << \"#{{{root}}}/extra_domain\"\n  end\nend\n"
        );
        let (app, diags) = diagnostics(&[
            ("config/application.rb", application_rb.as_str()),
            ("extra_domain/build_report.rb", INTERACTOR),
        ]);
        assert!(
            !interactor_error(&app, &diags),
            "`\"#{{{root}}}/extra_domain\"` names `extra_domain`; diagnostics = {diags:?}"
        );
    }
}
