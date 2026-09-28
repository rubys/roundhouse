//! A controller helper's keyword parameters reach the emitted `def`,
//! and a target that cannot express them says so.
//!
//! The ingest recorded them (#117) and the lowering dropped them, so
//! the `def` emitted with no parameters while the call site in the
//! same controller went on passing them by name — `ArgumentError` the
//! first time the action ran.
//!
//! Carried rather than converted. Ruby has keyword arguments; turning
//! them into positionals would lose the two things that make them
//! keywords, any order and skipping an optional one. A target that
//! cannot express the construct reports it instead of receiving a
//! lossy rewrite.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const CONTROLLER: &str = r#"class GaugesController < ApplicationController
  def index
    @out = label_for(code: params[:code].to_s, upcase: true)
  end

  private

  def label_for(code:, upcase: false)
    upcase ? code.upcase : code
  end
end
"#;

fn app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/gauges_controller.rb", CONTROLLER),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted_controller() -> String {
    ruby::emit_lowered_controllers(&app())
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with("gauges_controller.rb"))
        .map(|f| f.content)
        .expect("the controller is emitted")
}

#[test]
fn the_def_declares_what_the_call_site_passes() {
    let emitted = emitted_controller();
    assert!(
        emitted.contains("def label_for(code:, upcase: false)"),
        "got:\n{emitted}"
    );
    // The call site was always right; it is the `def` that was wrong.
    assert!(emitted.contains("label_for(code:"), "got:\n{emitted}");
}

#[test]
fn a_required_keyword_keeps_no_default() {
    // `code:` is required and `upcase:` is not. Emitting a default for
    // the required one would make a caller that omits it silently
    // succeed where Ruby raises.
    let emitted = emitted_controller();
    assert!(!emitted.contains("code: nil"), "got:\n{emitted}");
    assert!(emitted.contains("upcase: false"), "got:\n{emitted}");
}

#[test]
fn positional_and_optional_parameters_still_come_first() {
    // The guard on ordering: keywords are appended after the
    // positionals and their defaults, which is the only order Ruby
    // accepts.
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  def index\n    @out = mix(\"a\", \"b\", flag: true)\n  end\n\n  private\n\n  def mix(one, two = \"x\", flag: false)\n    flag ? one : two\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let emitted = ruby::emit_lowered_controllers(&app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with("gauges_controller.rb"))
        .map(|f| f.content)
        .expect("the controller is emitted");
    assert!(
        emitted.contains("def mix(one, two = \"x\", flag: false)"),
        "got:\n{emitted}"
    );
}

/// Every `keyword parameter` entry a target's emit produced.
fn ledgered(target: roundhouse::project::BuildTarget) -> Vec<String> {
    let app = app();
    // Through the real per-target entry point, so the exclusion list
    // is exercised rather than the predicate alone: spinel renders
    // these correctly because it uses the ruby emitter, and a blanket
    // "strict targets ledger it" would have warned on our own target.
    let (_, diags) = roundhouse::emit::diagnostics::scope(|| {
        let _ = roundhouse::project::target_files(&app, std::path::Path::new("."), target);
    });
    diags
        .iter()
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .filter(|d| d.contains("keyword parameter"))
        .collect()
}

#[test]
fn a_target_without_keyword_arguments_says_so() {
    let entries = ledgered(roundhouse::project::BuildTarget::Typescript);
    assert_eq!(entries.len(), 2, "one per keyword; got {entries:?}");
    assert!(entries.iter().any(|d| d.contains("`code`")), "got {entries:?}");
    assert!(entries.iter().any(|d| d.contains("`upcase`")), "got {entries:?}");
}

#[test]
fn the_ruby_family_does_not_report_what_it_renders() {
    // Including spinel, which is the point of checking by target
    // rather than by strictness: it compiles ruby, so `def
    // f(code:, upcase: false)` is exactly what it wants.
    for target in [
        roundhouse::project::BuildTarget::Ruby,
        roundhouse::project::BuildTarget::Spinel,
    ] {
        let entries = ledgered(target);
        assert!(entries.is_empty(), "{target:?} renders these; got {entries:?}");
    }
}
