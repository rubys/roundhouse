//! ActiveSupport's Hash core_ext — `symbolize_keys`,
//! `with_indifferent_access`, `to_query`, `first` — on a plain Hash.
//! Controllers reach for these on request and config hashes; each was a
//! `send_dispatch_failed` because the Hash table stopped at core Ruby.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn dispatch_failures(action: &str) -> Vec<String> {
    errors(action).into_iter().filter(|d| d.contains("send_dispatch_failed")).collect()
}

fn errors(action: &str) -> Vec<String> {
    let controller = format!(
        "class UsersController < ApplicationController\n  def index\n{action}\n  end\nend\n"
    );
    let files: Vec<(&str, String)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n".into()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n".into()),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n".into()),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n".into()),
        ("app/controllers/users_controller.rb", controller),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/users\", to: \"users#index\"\nend\n".into()),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.into_bytes())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .filter(|d| d.contains("error"))
        .collect()
}

#[test]
fn active_support_hash_methods_dispatch() {
    let diags = dispatch_failures("    h = { \"a\" => 1 }\n    h.symbolize_keys[:a]\n    h.deep_symbolize_keys\n    h.stringify_keys\n    h.with_indifferent_access\n    h.to_query.upcase\n    h.first\n    h.key(1)");
    assert!(diags.is_empty(), "{diags:?}");
}


#[test]
fn an_indifferent_hash_outside_its_modeled_surface_stays_an_error() {
    for (action, reported) in [
        ("    h = ActiveSupport::HashWithIndifferentAccess.new\n    h.is_a?(ActiveSupport::HashWithIndifferentAccess)", "HashWithIndifferentAccess"),
        ("    h = { \"a\" => 1 }.with_indifferent_access\n    h.transform_keys(&:upcase)", "transform_keys"),
        ("    h = { \"a\" => 1 }.with_indifferent_access\n    h.merge(\"b\" => 2) { |k, a, b| a + b }", "merge"),
    ] {
        let diags = errors(action);
        assert!(diags.iter().any(|d| d.contains(reported)), "{action}: {diags:?}");
    }
}
