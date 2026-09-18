//! `T.let(params[:date], String)` — sorbet-runtime's assertions, in
//! method bodies rather than in signatures.
//!
//! They evaluate to their first argument, so the value they wrap is the
//! value the program has. Before the unwrap, each one dispatched a
//! method on `T` — a module no gem in the catalog defines — which both
//! reported an error of its own and left everything downstream of the
//! assertion untyped: the `String` an app wrote down was the one thing
//! the analyzer could not use.
//!
//! The declared type is discarded here on purpose. Reading it as a
//! signature seed is a separate question with its own policy, and this
//! has to land without answering it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn tree(action: &str) -> HashMap<PathBuf, Vec<u8>> {
    let controller = format!(
        r#"class ReportsController < ApplicationController
  def show
    {action}
  end
end
"#
    );
    [
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "reports", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#
            .to_string(),
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string(),
        ),
        ("app/models/report.rb", "class Report < ApplicationRecord\nend\n".to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        ("app/controllers/reports_controller.rb", controller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#show\"\nend\n"
                .to_string(),
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect()
}

/// Lower first, then diagnose — the order the CLI runs them in.
fn diagnostics_for(action: &str) -> Vec<String> {
    let mut app = ingest_app_from_tree(tree(action)).expect("ingest");
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .collect()
}

/// A dispatch on `T` itself — not on `Turbo::…` or any other constant
/// that merely starts with the letter.
fn dispatches_on_t(diagnostic: &str) -> bool {
    diagnostic.trim_end().ends_with("on T")
}

#[test]
fn t_let_carries_its_arguments_type_downstream() {
    // `upcase` resolves only if `date` is the Str that `params[:date]`
    // is; an unresolved receiver is what the assertion used to produce.
    let diags = diagnostics_for("date = T.let(params[:date], String)\n    @slug = date.upcase");
    assert!(
        !diags.iter().any(|d| dispatches_on_t(d)),
        "`T.let` must not dispatch on the unmodeled `T` module; diagnostics = {diags:?}"
    );
    assert!(
        !diags.iter().any(|d| d.contains("send_dispatch_failed") && d.contains("`upcase`")),
        "the wrapped expression's type has to survive the assertion; diagnostics = {diags:?}"
    );
}

#[test]
fn the_other_assertion_forms_unwrap_the_same_way() {
    for action in [
        "@slug = T.must(params[:date]).upcase",
        "@slug = T.cast(params[:date], String).upcase",
        "@slug = T.must_because(params[:date]) { \"routed\" }.upcase",
        "@slug = T.unsafe(params[:date]).upcase",
        "@slug = T.assert_type!(params[:date], String).upcase",
    ] {
        let diags = diagnostics_for(action);
        assert!(
            !diags.iter().any(|d| dispatches_on_t(d)),
            "{action} still dispatches on `T`; diagnostics = {diags:?}"
        );
    }
}

#[test]
fn a_method_named_let_on_something_else_is_untouched() {
    // The arm keys on the `T` receiver, not on the method name: a
    // `let` somewhere else is an ordinary call and keeps its dispatch.
    let diags = diagnostics_for("@slug = Report.let(params[:date])");
    assert!(
        diags.iter().any(|d| d.contains("send_dispatch_failed") && d.contains("`let`")),
        "only `T.let` unwraps; diagnostics = {diags:?}"
    );
}
