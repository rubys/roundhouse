//! A view module named like the model it renders: lobsters'
//! `Views::Search.index_into(io, search, …)` takes a `Search`, and an
//! RBS type name resolves lexically the way a Ruby constant does, so
//! inside `module Views; module Search` a bare `Search` is the view
//! module itself. Spinel typed `search` as the module and refused
//! `search.total_results > -1` ("unsupported comparison … recv=ty0")
//! the day the `_into` variant started declaring its model parameter.
//! The sidecar writes the colliding name rooted, `::Search`, which
//! names the model from anywhere; a name no enclosing segment shadows
//! is written as before.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn sidecar(view_dir: &str) -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"stories\", force: :cascade do |t|\n    t.string \"title\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            format!("Rails.application.routes.draw do\n  get \"/{view_dir}\", to: \"{view_dir}#index\"\nend\n").into_bytes(),
        ),
        (
            PathBuf::from("app/models/search.rb"),
            // Tableless, as lobsters' is: `ActiveModel::Validations` is
            // what makes it a model the view's parameter is typed by.
            b"class Search\n  include ActiveModel::Validations\n\n  attr_accessor :total_results\n\n  def initialize\n    @total_results = -1\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from(format!("app/controllers/{view_dir}_controller.rb")),
            format!("class {}Controller < ApplicationController\n  def index\n    @search = Search.new\n  end\nend\n", capitalize(view_dir)).into_bytes(),
        ),
        (
            PathBuf::from(format!("app/views/{view_dir}/index.html.erb")),
            b"<% if @search.total_results > -1 %>\n  <p><%= @search.total_results %> results</p>\n<% end %>\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let suffix = format!("sig/app/views/{view_dir}/index.rbs");
    ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with(&suffix))
        .unwrap_or_else(|| panic!("no {suffix} emitted"))
        .content
        .clone()
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[test]
fn a_model_named_like_its_view_module_is_written_rooted() {
    let rbs = sidecar("search");
    assert!(rbs.contains("module Views\n  module Search\n"), "{rbs}");
    assert!(
        rbs.contains("def self.index_into: (String io, ::Search search"),
        "the model parameter must be rooted past the enclosing `Search` module:\n{rbs}"
    );
}

#[test]
fn a_model_no_enclosing_segment_shadows_is_written_bare() {
    let rbs = sidecar("lookup");
    assert!(
        rbs.contains("def self.index_into: (String io, Search search"),
        "nothing shadows `Search` under `Views::Lookup`:\n{rbs}"
    );
    assert!(!rbs.contains("::Search"), "{rbs}");
}
