//! A controller helper's keyword parameters are read, and a
//! declaration types them.
//!
//! They are the case where a declaration is the ONLY possible source
//! of a type. A positional parameter is observed from its call sites;
//! a keyword argument is passed by NAME, so there is no index an
//! observation could be read from.
//!
//! The EMIT half is deliberately not here. Carrying these into the
//! emitted `def` fixes the ruby family — where the `def` currently
//! takes nothing while the call site passes them by name — and breaks
//! the others, whose emitters render a keyword CALL as one hash
//! argument and would then meet a definition taking two positionals.
//! That needs a per-target answer rather than a lowering, and is
//! filed on its own.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;

fn app_with(controller: &str) -> roundhouse::App {
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
        ("app/controllers/gauges_controller.rb", controller),
        ("app/views/gauges/index.html.erb", "<p><%= @out %></p>\n"),
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

const CONTROLLER: &str = r#"class GaugesController < ApplicationController
  def index
    @out = label_for(code: params[:code].to_s, upcase: true)
  end

  private

  sig { params(code: String, upcase: T::Boolean).returns(String) }
  def label_for(code:, upcase: false)
    upcase ? code.upcase : code
  end
end
"#;

#[test]
fn the_declaration_types_them_because_nothing_else_can() {
    // A keyword argument is passed by name, so the call-site
    // observations — which are positional — have nothing to say about
    // it. The `sig` above the `def` is the only source.
    let app = app_with(CONTROLLER);
    let unresolved: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::UnresolvedType { name: Some(name), .. } => {
                Some(name.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    assert!(
        !unresolved.iter().any(|n| n == "code" || n == "upcase"),
        "the declared keyword types should reach the body; unresolved = {unresolved:?}"
    );
}
