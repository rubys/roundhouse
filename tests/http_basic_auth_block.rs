//! `authenticate_or_request_with_http_basic` yields two values, and
//! both of them are typed.
//!
//! The registry could model a block that yields ONE value. A block
//! yielding two — the username and the password an authenticator
//! compares — bound only the first, so the second read as `Var` and
//! everything compared against it went untyped with it.
//!
//! Generalized at the lookup rather than special-cased here: a block
//! type that names several parameters spreads them.

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
        ("app/controllers/application_controller.rb", controller),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  def index\n  end\nend\n",
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

const CONTROLLER: &str = r#"class ApplicationController < ActionController::Base
  def authenticate
    expected = "admin"
    authenticate_or_request_with_http_basic do |username, password|
      username == expected && password.length > 3
    end
  end
end
"#;

#[test]
fn both_yielded_values_are_bound() {
    let app = app_with(CONTROLLER);
    let unresolved = unresolved(&app);
    for name in ["authenticate_or_request_with_http_basic", "username", "password"] {
        assert!(
            !unresolved.iter().any(|n| n == name),
            "`{name}` should resolve; unresolved = {unresolved:?}"
        );
    }
}

#[test]
fn the_second_one_is_a_string_not_an_escape_hatch() {
    // The half that matters: binding the name is not enough if it
    // binds to nothing useful. `password.length` has to dispatch on
    // String — a method off that table still reports.
    let controller = r#"class ApplicationController < ActionController::Base
  def authenticate
    authenticate_or_request_with_http_basic do |username, password|
      password.definitely_not_a_string_method
    end
  end
end
"#;
    let app = app_with(controller);
    let failures: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { method, recv_ty } => {
                Some(format!("{}#{}", roundhouse::ide::render_ty(&recv_ty), method.as_str()))
            }
            _ => None,
        })
        .collect();
    assert!(
        failures
            .iter()
            .any(|f| f == "String#definitely_not_a_string_method"),
        "the second parameter is a String; failures = {failures:?}"
    );
}

#[test]
fn a_single_parameter_block_is_unchanged() {
    // The guard on the generalization: a registry block that yields
    // one value must keep binding that one value, not a list.
    let controller = r#"class ApplicationController < ActionController::Base
  def render_it
    respond_to do |format|
      format.html
    end
  end
end
"#;
    let app = app_with(controller);
    let unresolved = unresolved(&app);
    assert!(
        !unresolved.iter().any(|n| n == "format"),
        "a one-value block still binds its value; unresolved = {unresolved:?}"
    );
}
