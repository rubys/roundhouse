//! A template reads `request`, the same object the controller does.
//!
//! Rails' view context delegates it, so a layout asking for
//! `request.content_security_policy_nonce` — or a manifest template
//! building a URL from `request.base_url` — is ordinary code. It had
//! no type here: the controller's registration is class-side, and a
//! template dispatches instance-side against the view context.

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};

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

fn tree(template: &str) -> Vec<(&'static str, String)> {
    vec![
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
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  def index\n  end\nend\n".to_string(),
        ),
        ("app/views/gauges/index.html.erb", template.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n".to_string(),
        ),
    ]
}

fn app_with(template: &str) -> roundhouse::App {
    let owned = tree(template);
    let borrowed: Vec<(&str, &str)> = owned.iter().map(|(p, c)| (*p, c.as_str())).collect();
    app_from(&borrowed)
}

#[test]
fn a_template_reading_request_resolves() {
    let app = app_with("<p><%= request.base_url %></p>\n");
    let unresolved = unresolved(&app);
    assert!(
        !unresolved.iter().any(|n| n == "request"),
        "`request` should resolve in a template; unresolved = {unresolved:?}"
    );
}

#[test]
fn the_csp_nonce_a_layout_interpolates_is_on_that_table() {
    // The method that made the point: typing `request` turned this
    // read from an untyped call into a dispatch failure, because the
    // request table did not carry it. Nil until a policy sets one —
    // Rails implements it as `get_header(NONCE)`.
    let app = app_with(
        "<script nonce=\"<%= request.content_security_policy_nonce %>\"></script>\n",
    );
    let failures: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::SendDispatchFailed { method, .. } => {
                Some(method.as_str().to_string())
            }
            _ => None,
        })
        .collect();
    assert!(
        !failures.iter().any(|m| m == "content_security_policy_nonce"),
        "the nonce is a real method on the request; failures = {failures:?}"
    );
}

#[test]
fn it_is_the_same_object_the_controller_has() {
    // Not a bare "something answers": the surface behind it is
    // ActionDispatch::Request's, so a method off that table still
    // reports rather than going quietly gradual.
    let app = app_with("<p><%= request.definitely_not_a_request_method %></p>\n");
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
            .any(|f| f == "ActionDispatch::Request#definitely_not_a_request_method"),
        "the receiver is the request, so an unknown method reports on it; got {failures:?}"
    );
}
