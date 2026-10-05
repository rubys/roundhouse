//! `get "up" => "rails/health#show"` — the `rails new` health check —
//! routes to Rails' own `Rails::HealthController`, which no app tree
//! holds. Ingest synthesizes it; the targets that emit a namespaced
//! controller serve `/up`, the rest are left as they were without it.
//! The emitted-program half is kept out of tests/emit_and_run.rs so
//! concurrent appends there do not conflict (same harness); the native
//! Spinel half is in tests/spinel_toolchain.rs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::diagnostic::Severity;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// It dispatched to nothing: `/up` answered 404, so a deploy proxy
/// probing it never saw the app as healthy.
#[test]
fn the_rails_health_check_answers_up() {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"up\" => \"rails/health#show\", as: :rails_health_check\n",
        )
        .run_ruby(r#"
out = StringIO.new
Main.run({ "REQUEST_METHOD" => "GET", "PATH_INFO" => "/up", "HTTP_ACCEPT" => "text/html" }, StringIO.new(""), out)
up = out.string
raise "/up:\n#{up}" unless up.start_with?("Status: 200") && up.include?('<body style="background-color: green"></body>')
puts "up"
"#)
        .assert_passes();
}

const HEALTH: &str = "Rails::HealthController";

/// A one-controller app with the health route; `synthesized: false`
/// drops the synthesized controller before analysis, which is the app
/// every target saw before ingest wrote it.
fn app(synthesized: bool) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    head :no_content\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: :index\n  get \"up\" => \"rails/health#show\", as: :rails_health_check\nend\n",
        ),
        (
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
    ]
    .iter()
    .map(|(path, src)| (PathBuf::from(path), src.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    assert!(app.controllers.iter().any(|c| c.name.0.as_str() == HEALTH), "ingest synthesizes {HEALTH}");
    if !synthesized {
        app.controllers.retain(|c| c.name.0.as_str() != HEALTH);
    }
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

/// The target's files, and the warnings that name the health controller.
fn emit(app: &roundhouse::App, target: BuildTarget) -> (Vec<(String, String)>, Vec<String>) {
    let (files, diags) =
        roundhouse::emit::diagnostics::scope(|| target_files(app, Path::new("."), target));
    let files = files.unwrap_or_else(|e| panic!("{target:?}: {e}"));
    let warnings = diags
        .into_iter()
        .filter(|d| d.severity == Severity::Warning && d.message.contains(HEALTH))
        .map(|d| d.message)
        .collect();
    (files, warnings)
}

/// TypeScript wrote the class name into an import, a file name and an
/// object key (`import { Rails::HealthController } from
/// "./app/controllers/rails::health_controller.js"`), a syntax error for
/// the whole app.
#[test]
fn the_typescript_tree_names_no_rails_namespace() {
    let (files, warnings) = emit(&app(true), BuildTarget::Typescript);
    for (path, content) in &files {
        for needle in ["Rails::", "rails::"] {
            assert!(!path.contains(needle), "{path}");
            assert!(!content.contains(needle), "{path} contains {needle}");
        }
    }
    assert_eq!(warnings.len(), 1, "the 404 is ledgered: {warnings:?}");
}

/// Every target that does not emit a namespaced controller gets exactly
/// the tree it got before the synthesis, plus a warning saying `/up`
/// still answers 404 there.
#[test]
fn targets_without_namespaced_controllers_emit_as_before() {
    let (with, without) = (app(true), app(false));
    for &target in BuildTarget::TRANSPILE {
        if matches!(target, BuildTarget::Ruby | BuildTarget::Jruby | BuildTarget::Spinel) {
            continue;
        }
        // Kotlin's first emit on a thread differs from the next one
        // (`record.errors` vs `record.errors()` in Errors.kt), with or
        // without this controller, so compare warm emits.
        emit(&without, target);
        let (before, _) = emit(&without, target);
        let (files, warnings) = emit(&with, target);
        for ((path, content), (before_path, before_content)) in files.iter().zip(&before) {
            assert_eq!(path, before_path, "{target:?}");
            assert!(content == before_content, "{target:?} {path}:\n{content}\n--- without the synthesized controller ---\n{before_content}");
        }
        assert_eq!(files.len(), before.len(), "{target:?}");
        assert_eq!(warnings.len(), 1, "{target:?}: {warnings:?}");
    }
}

/// JRuby ships the Ruby tree's app code; the CRuby run above is its
/// evidence. No warning on the targets that serve `/up`.
#[test]
fn jruby_carries_the_ruby_trees_health_controller() {
    let app = app(true);
    let (ruby, ruby_warnings) = emit(&app, BuildTarget::Ruby);
    let (jruby, jruby_warnings) = emit(&app, BuildTarget::Jruby);
    assert!(ruby_warnings.is_empty() && jruby_warnings.is_empty());
    let file = |files: &[(String, String)], path: &str| {
        files.iter().find(|(p, _)| p == path).map(|(_, c)| c.clone())
            .unwrap_or_else(|| panic!("{path} not emitted"))
    };
    for path in ["app/controllers/rails/health_controller.rb", "config/routes.rb", "main.rb"] {
        assert_eq!(file(&ruby, path), file(&jruby, path), "{path}");
    }
    assert!(file(&ruby, "main.rb").contains("when :rails_health then"));
}

