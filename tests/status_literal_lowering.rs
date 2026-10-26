//! `render …, status: 400` reaches the runtime as `status: :bad_request`.
//!
//! `lower::status_literal` retains the canonical Symbol shape for known
//! literal codes; computed Integers pass through the Ruby/Spinel runtime resolver.
//! A JSON payload's `status:` key is data and stays an Integer.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn integer_statuses_become_the_tables_symbols() {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/a\" => \"things#a\"\n  get \"/b\" => \"things#b\"\n  get \"/c\" => \"things#c\"\nend\n",
        ),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  def a\n    render plain: \"no\", status: 400\n  end\n\n  def b\n    head 404\n  end\n\n  def c\n    render json: {error: \"bad\", status: 400}\n  end\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("app/controllers/things_controller.rb"))
        .expect("controller emitted")
        .content
        .clone();
    assert!(src.contains("status: :bad_request"), "render's Integer status grounded:\n{src}");
    assert!(src.contains("head(:not_found") || src.contains("head :not_found"), "head's positional status grounded:\n{src}");
    assert!(src.contains("status: 400 }") || src.contains("status: 400}"), "a JSON body's status key stays data:\n{src}");
}
