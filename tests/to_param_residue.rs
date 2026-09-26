//! An interpolated `to_param` on a receiver inference could not type is
//! routed to the runtime's `ActiveSupport.to_param(value)`; one on a
//! typed record keeps its own method.
//!
//! lobsters' anonymous story-list cache key is the shape —
//! `opts.merge(page: page).sort.map { |k, v| "#{k}=#{v.to_param}" }` over
//! `true`, an Integer and a Hash — and spinel's dispatch for the name had
//! arms for the app's records only, so every anonymous story list 500'd.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn untyped_receivers_go_to_the_runtime_typed_ones_keep_their_call() {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"stories\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n",
        ),
        ("app/models/story.rb", "class Story < ApplicationRecord\nend\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/a\" => \"things#a\"\nend\n",
        ),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  def a\n    story = Story.find(1)\n    key = cache_opts.map { |k, v| \"#{k}=#{v.to_param}\" }.join(\" \")\n    render plain: \"#{key} #{story.to_param}\"\n  end\n\n  private\n\n  def cache_opts(opts = {})\n    opts.merge(top: true, page: 1)\n  end\nend\n",
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
    assert!(src.contains("#{ActiveSupport.to_param(v)}"), "untyped hash value routed:\n{src}");
    assert!(src.contains("#{story.to_param}"), "a typed record keeps its own to_param:\n{src}");
}
