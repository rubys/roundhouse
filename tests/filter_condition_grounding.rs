//! A controller filter's `if:` / `unless:` lambda is an app body: it is
//! spliced into the dispatcher as written, so the shared hook passes
//! have to see it — `lower::blank` among them.
//!
//! lobsters' `around_action :track_story_reads, only: [:show], if: -> {
//! @user.present? }` reached spinel as a bare `@user.present?`, and every
//! story page 500'd there (`undefined method 'present?'` for a User, or
//! for nil when anonymous). Model callback conditions were already
//! walked; controller filter conditions were not.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn a_filter_condition_is_blank_grounded() {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/s\" => \"stories#show\"\nend\n"),
        (
            "app/controllers/stories_controller.rb",
            "class StoriesController < ApplicationController\n  around_action :track, only: [:show], if: -> { @user.present? }\n  before_action :note, unless: -> { @user.blank? }\n\n  def show\n    render plain: \"ok\"\n  end\n\n  private\n\n  def track\n    yield\n  end\n\n  def note\n  end\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("app/controllers/stories_controller.rb"))
        .expect("controller emitted")
        .content
        .clone();
    assert!(!src.contains("@user.present?"), "if: condition grounded:\n{src}");
    assert!(!src.contains("@user.blank?"), "unless: condition grounded:\n{src}");
}

/// The same untyped `present?` in a VIEW, through symbol-to-proc: lobsters'
/// `_singledetail` builds its story classes as `[:story, klass,
/// cond && "upvoted", …].filter(&:present?)` — Symbols, Strings and
/// `false`, a `present?` send on a Symbol on spinel. The view predicate
/// rewrite routes the whole-body `|x| x.present?` block to the runtime
/// helper (the `.empty?` form it uses elsewhere is wrong for a Symbol).
#[test]
fn a_symbol_to_proc_present_in_a_view_is_grounded() {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/s\" => \"stories#show\"\nend\n"),
        (
            "app/controllers/stories_controller.rb",
            "class StoriesController < ApplicationController\n  def show\n    @up = true\n  end\nend\n",
        ),
        (
            "app/views/stories/show.html.erb",
            "<% classes = [:story, @up && \"upvoted\"].filter(&:present?) %>\n<div class=\"<%= classes.join(\" \") %>\"></div>\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("app/views/stories/show.rb"))
        .expect("view emitted")
        .content
        .clone();
    assert!(src.contains("ActiveSupport.present?(x)"), "grounded to the runtime helper:\n{src}");
    assert!(!src.contains("(&:present?)"), "no Symbol#present? dispatch left:\n{src}");
}
