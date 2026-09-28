//! `Model.human_attribute_name(:col)` resolves to a String.
//!
//! It is the ActiveModel translation entry point every form label and
//! table header goes through — stock Rails, no gem involved — and the
//! model class-method registry did not carry it, so each call read as a
//! dispatch failure. On a real Rails 8 app (Telescope, 255 models/views)
//! this single missing entry accounted for 359 of 836 error diagnostics.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

const FILES: &[(&str, &str)] = &[
    (
        "db/schema.rb",
        "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n",
    ),
    ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
    (
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::Base\nend\n",
    ),
    (
        "app/controllers/users_controller.rb",
        "class UsersController < ApplicationController\n  def index\n    @users = User.all\n  end\nend\n",
    ),
    (
        "app/views/users/index.html.erb",
        "<th><%= User.human_attribute_name(:name) %></th>\n",
    ),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  resources :users\nend\n",
    ),
];

fn analyzed_app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> =
        FILES.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

#[test]
fn human_attribute_name_dispatches_and_is_a_string() {
    let app = analyzed_app();
    let failures: Vec<_> = diagnose(&app)
        .iter()
        .filter(|d| d.code() == "send_dispatch_failed")
        .map(|d| format!("{d:?}"))
        .collect();
    assert!(failures.is_empty(), "unexpected dispatch failures: {failures:?}");

    let view = app.views.first().expect("view ingested");
    let body = format!("{:?}", view.body);
    assert!(
        body.contains("human_attribute_name"),
        "the view should carry the call:\n{body}"
    );
    // Not a gradual escape either: an `untyped` answer would surface as
    // a `gradual_untyped` warning at the call.
    let gradual: Vec<_> = diagnose(&app)
        .iter()
        .filter(|d| d.code() == "gradual_untyped")
        .map(|d| format!("{d:?}"))
        .collect();
    assert!(gradual.is_empty(), "the label is a String, not untyped: {gradual:?}");
}
