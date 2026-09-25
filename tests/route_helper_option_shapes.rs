//! Two ways lobsters hands a route helper its options, both of which
//! reached the generated helper bound to the wrong slot.
//!
//! `get "/s/:id/(:title)" => "stories#show", :as => "story_short_id"`
//! generates `story_short_id_path(id, title = nil, anchor: nil)`. Rails
//! fills a path param from the option hash as readily as from a
//! positional, and lobsters' `Routes` (a `class << self` of link
//! builders) does both:
//!
//! ```ruby
//! def title_path story, anchor: nil
//!   story_short_id_path(story, title: story.title_as_slug, anchor:)
//! end
//!
//! def comment_target_path comment, title = false
//!   options = {anchor: "c_#{comment.short_id}"}
//!   options[:title] = … if title
//!   story_short_id_path(story, options)
//! end
//! ```
//!
//! and a controller calls `Routes.title_path(story, anchor: a)` against
//! the keyword `title_path` ingest flattened to `(story, anchor = nil)`.
//! Before: `title:` was an unknown keyword (ArgumentError on every
//! story link), the `options` Hash became the title (`/s/x/{anchor: …}`),
//! and the controller's `{anchor: a}` became the anchor.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const ROUTES_CLASS: &str = r#"class LinkPaths
  class << self
    def title_path story, anchor: nil
      story_short_id_path(story.short_id, title: story.title, anchor:)
    end

    def comment_target_path story, title = false
      options = {anchor: "c_1"}
      options[:title] = story.title if title
      story_short_id_path(story.short_id, options)
    end
  end
end
"#;

fn emitted() -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"stories\", force: :cascade do |t|\n    t.string \"short_id\", null: false\n    t.string \"title\", null: false\n  end\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/story.rb", "class Story < ApplicationRecord\nend\n"),
        ("app/models/link_paths.rb", ROUTES_CLASS),
        (
            "app/controllers/stories_controller.rb",
            "class StoriesController < ActionController::Base\n  def show\n    story = Story.find(1)\n    redirect_to LinkPaths.title_path(story, anchor: \"top\")\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/s/:id/(:title)\" => \"stories#show\", :as => \"story_short_id\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .into_iter()
        .chain(ruby::emit_lowered_models(&app))
        .chain(ruby::emit_lowered_controllers(&app))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_named_path_param_moves_into_its_slot() {
    let out = emitted();
    assert!(
        out.contains("story_short_id_path(story.short_id, story.title, anchor: anchor)"),
        "got:\n{out}"
    );
}

#[test]
fn a_local_options_hash_is_spread_into_the_slots_it_names() {
    let out = emitted();
    assert!(
        out.contains("story_short_id_path(story.short_id, options[:title], anchor: options[:anchor])"),
        "got:\n{out}"
    );
}

#[test]
fn a_class_method_keyword_call_fills_the_flattened_slot() {
    let out = emitted();
    assert!(out.contains("LinkPaths.title_path(story, \"top\")"), "got:\n{out}");
}
