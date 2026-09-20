//! A declared parameter type reaches the body it was declared above.
//!
//! The signature readers land a method's `Ty::Fn` in the dispatch
//! table, which types the method for its CALLERS. The body was seeded
//! from call sites alone, so the one place the annotation is written —
//! directly above the `def` whose parameters it names — was the one
//! place it did not reach.
//!
//! Only two tests here, deliberately. Several obvious-looking ones
//! passed with the change reverted: a positional parameter whose call
//! site is typed was already seeded from that observation, and a class
//! nothing calls carries no diagnostics to read. Neither proved
//! anything, so neither is here. The keyword-parameter case — where
//! there IS no observation to fall back on — lives in
//! `controller_keyword_params`.

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};
use roundhouse::ty::Ty;

fn app_from(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn unresolved(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::UnresolvedType { name: Some(name), .. } => {
                Some(name.as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

fn base(extra: &[(&'static str, String)]) -> Vec<(&'static str, String)> {
    let mut files: Vec<(&'static str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n".to_string(),
        ),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string(),
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n".to_string(),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n".to_string(),
        ),
    ];
    files.extend(extra.iter().cloned());
    files
}

fn app_with(extra: &[(&'static str, String)]) -> roundhouse::App {
    let owned = base(extra);
    let borrowed: Vec<(&str, &str)> = owned.iter().map(|(p, c)| (*p, c.as_str())).collect();
    app_from(&borrowed)
}

#[test]
fn the_declared_type_is_the_one_the_body_gets() {
    // Not merely "something resolves": the body sees the type the
    // signature named, so what is computed from it is typed too —
    // which is the whole point of reading the annotation.
    let service = r#"class Formatter
  def label_for(code)
    code.upcase
  end
end
"#;
    let app = app_with(&[
        ("app/services/formatter.rb", service.to_string()),
        (
            "sig/formatter.rbs",
            "class Formatter\n  def label_for: (String code) -> String\nend\n".to_string(),
        ),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  def index\n  end\nend\n".to_string(),
        ),
    ]);
    let ty = roundhouse::ide::type_at_position(
        &app,
        "app/services/formatter.rb",
        roundhouse::ide::Position { line: 2, character: 4 },
    )
    .and_then(|t| t.ty);
    assert_eq!(ty, Some(Ty::Str), "the body reads `code` as the declared String");
}

#[test]
fn an_observed_type_still_wins_where_there_is_one() {
    // The additive half. A sidecar that declares something the call
    // sites contradict must not take over a body that already typed:
    // this change fills gaps, it does not re-decide.
    let service = r#"class Counter
  def bump(step)
    step + 1
  end
end
"#;
    let app = app_with(&[
        ("app/services/counter.rb", service.to_string()),
        (
            "sig/counter.rbs",
            "class Counter\n  def bump: (String step) -> Integer\nend\n".to_string(),
        ),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  def index\n    @n = Counter.new.bump(1)\n  end\nend\n".to_string(),
        ),
    ]);
    // The call site passes an Integer; that observation is what the
    // body is seeded with, so `step + 1` is Integer arithmetic and no
    // String dispatch failure appears.
    let failures: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { recv_ty, .. } => {
                Some(roundhouse::ide::render_ty(&recv_ty))
            }
            _ => None,
        })
        .collect();
    assert!(
        !failures.iter().any(|f| f == "String"),
        "the observed Integer should still win over the declared String; failures = {failures:?}"
    );
    let _ = Ty::Int;
}
