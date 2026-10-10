//! Rust emission for explicit nil guards on repeated nilable expressions.

use roundhouse::emit::{EmittedFile, rust};
use roundhouse::ingest::ingest_app_from_tree;
use std::collections::HashMap;
use std::path::PathBuf;

fn emitted() -> Vec<EmittedFile> {
    let schema = "ActiveRecord::Schema.define do\n  create_table :users do |t|\n    t.string :name\n  end\nend\n";
    let user = "class User < ApplicationRecord\nend\n";
    let controller = r#"
class RoomsController < ApplicationController
  def current_user_meta_tags
    unless User.find_by(id: 1).nil?
      [User.find_by(id: 1).id, User.find_by(id: 1).name]
    end
  end
end
"#;
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", schema),
        ("app/models/user.rb", user),
        ("app/controllers/rooms_controller.rb", controller),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]
    .into_iter()
    .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    rust::emit(&app)
}

fn file(files: &[EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .unwrap_or_else(|| {
            panic!(
                "missing {suffix}; emitted {:?}",
                files.iter().map(|f| &f.path).collect::<Vec<_>>()
            )
        })
        .content
        .clone()
}

#[test]
fn nil_guard_refines_each_repeated_read_without_caching_it() {
    let helper = file(&emitted(), "rooms_controller.rs");
    let start = helper
        .find("pub fn current_user_meta_tags(")
        .expect("helper method emitted");
    let body = &helper[start..];
    let body = body.split("\n    pub fn ").next().unwrap_or(body);

    assert!(
        body.contains("if !(User::find_by") && body.contains(".is_none())"),
        "nil guard must retain its original test:\n{body}"
    );
    assert!(
        body.contains("unwrap().id()"),
        "id reads are refined to the non-nil receiver:\n{body}"
    );
    assert!(
        body.contains("unwrap().name()"),
        "name reads are refined to the non-nil receiver:\n{body}"
    );
    assert_eq!(
        body.matches("User::find_by").count(),
        3,
        "preserve condition and both getter evaluations:\n{body}"
    );
    assert_eq!(body.matches(".unwrap()").count(), 2, "{body}");
}
