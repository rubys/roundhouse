//! A view whose namespace shadows a top-level app class.
//!
//! Views emit as `module Views\n  module <Dir>`, so an unqualified
//! `Stats.get_cached_graph(:users)` inside `app/views/stats/index.html.erb`
//! resolves LEXICALLY to `Views::Stats` — the view module itself — rather
//! than the top-level `Stats` class the template meant. Ruby finds the
//! inner constant first, so the call raises `NoMethodError: undefined
//! method 'get_cached_graph' for module Views::Stats`.
//!
//! This bit the CRuby target as much as the AOT one (GET /stats on
//! lobsters); the spinel lane just surfaced it at compile time instead
//! of at request time.

use roundhouse::emit::ruby::emit_lowered_views;
use roundhouse::ingest::ingest_app_from_tree;

fn emit_views(files: Vec<(&str, &str)>) -> String {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    emit_lowered_views(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :stories do |t|\n    t.string :title\n  end\nend\n";

#[test]
fn a_shadowed_class_reference_is_rooted() {
    let out = emit_views(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/stats.rb",
            "class Stats\n  def self.cached_graph(name)\n    name.to_s\n  end\nend\n",
        ),
        (
            "app/views/stats/index.html.erb",
            "<%= Stats.cached_graph(:users) %>\n",
        ),
    ]);
    assert!(
        out.contains("::Stats.cached_graph"),
        "the view's own namespace shadows Stats, so the reference must be rooted:\n{out}"
    );
}

#[test]
fn an_unshadowed_class_reference_is_left_alone() {
    // `Story` doesn't collide with the `Views::Stats` namespace, so
    // nothing about it needs rooting — the rewrite must stay narrow
    // rather than blanket-rooting every constant in every view.
    let out = emit_views(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/story.rb",
            "class Story < ApplicationRecord\n  def self.newest_title\n    \"t\"\n  end\nend\n",
        ),
        (
            "app/views/stats/index.html.erb",
            "<%= Story.newest_title %>\n",
        ),
    ]);
    assert!(
        out.contains("Story.newest_title"),
        "expected the call to survive:\n{out}"
    );
    assert!(
        !out.contains("::Story"),
        "an unshadowed constant must not be rooted:\n{out}"
    );
}

#[test]
fn a_view_directory_with_no_matching_class_roots_nothing() {
    // `Views::Home` shadows nothing (there is no `Home` class), so a
    // reference to some other class stays exactly as written.
    let out = emit_views(vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/stats.rb",
            "class Stats\n  def self.cached_graph(name)\n    name.to_s\n  end\nend\n",
        ),
        (
            "app/views/home/index.html.erb",
            "<%= Stats.cached_graph(:users) %>\n",
        ),
    ]);
    assert!(
        out.contains("Stats.cached_graph"),
        "expected the call to survive:\n{out}"
    );
    assert!(
        !out.contains("::Stats"),
        "no shadowing here, so nothing should be rooted:\n{out}"
    );
}
