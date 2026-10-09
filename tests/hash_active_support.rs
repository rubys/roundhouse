//! ActiveSupport's Hash core_ext — `symbolize_keys`,
//! `with_indifferent_access`, `to_query`, `first` — on a plain Hash.
//! Controllers reach for these on request and config hashes; each was a
//! `send_dispatch_failed` because the Hash table stopped at core Ruby.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn dispatch_failures(action: &str) -> Vec<String> {
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
        .filter(|d| d.contains("send_dispatch_failed"))
        .collect()
}

#[test]
fn active_support_hash_methods_dispatch() {
    let diags = dispatch_failures("    h = { \"a\" => 1 }\n    h.symbolize_keys[:a]\n    h.deep_symbolize_keys\n    h.stringify_keys\n    h.with_indifferent_access\n    h.to_query.upcase\n    h.first\n    h.key(1)");
    assert!(diags.is_empty(), "{diags:?}");
}

#[test]
fn rust_helper_hash_merge_resolves_the_shipped_runtime_function() {
    let tree = [(
        PathBuf::from("app/helpers/merge_helper.rb"),
        b"module MergeHelper\n  def attrs\n    { left: \"old\", right: \"kept\" }.merge(left: \"new\")\n  end\nend\n".to_vec(),
    )]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = roundhouse::emit::rust::emit(&app);
    let helper = files
        .iter()
        .find(|file| file.path.ends_with("merge_helper_class.rs"))
        .expect("helper output");
    assert!(helper.content.contains("merge_attrs("), "{}", helper.content);
    assert!(
        helper.content.contains("use crate::hash_ext::merge_attrs;"),
        "the helper must bind the existing runtime function:\n{}",
        helper.content
    );
    assert!(files.iter().any(|file| file.path.ends_with("hash_ext.rs")));
}
