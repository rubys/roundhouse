//! A method whose return values are class constants returns the CLASS
//! (#132). lobsters' `Search#searched_model` answers `Story` or
//! `Comment`; the sidecar declared `-> (Comment | Story)`, spinel
//! trusted it, and `searched_model.none` compiled to a run-time
//! NoMethodError. The sidecar must say `singleton(...)`, and only for a
//! tail made entirely of class constants.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
  end
  create_table "comments", force: :cascade do |t|
    t.string "body", null: false
  end
end
"#;

const SEARCH: &str = r#"class Search
  LIMIT = 20

  def initialize(what)
    @what = what
  end

  def searched_model
    if @what == :stories
      Story
    else
      Comment
    end
  end

  def only_stories
    Story
  end

  def limit
    LIMIT
  end

  def model_or_record
    if @what == :stories
      Story
    else
      Comment.new
    end
  end

  def results
    searched_model.none
  end
end
"#;

fn search_rbs() -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("app/models/story.rb", "class Story < ApplicationRecord\nend\n"),
        ("app/models/comment.rb", "class Comment < ApplicationRecord\nend\n"),
        ("app/models/search.rb", SEARCH),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_library(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("search.rbs"))
        .map(|f| f.content)
        .expect("search.rbs sidecar")
}

#[test]
fn class_constant_tail_is_a_singleton() {
    let rbs = search_rbs();
    assert!(
        rbs.contains("def searched_model: () -> (singleton(Story) | singleton(Comment))"),
        "{rbs}"
    );
    assert!(rbs.contains("def only_stories: () -> singleton(Story)"), "{rbs}");
}

#[test]
fn value_constants_and_mixed_tails_keep_their_types() {
    let rbs = search_rbs();
    assert!(rbs.contains("def limit: () -> Integer"), "{rbs}");
    assert!(!rbs.contains("model_or_record: () -> singleton"), "{rbs}");
}

#[test]
fn a_call_on_the_returned_class_resolves_like_one_on_the_constant() {
    // `searched_model.none` dispatches on each class, and the two
    // relations it answers render as one RBS member.
    let rbs = search_rbs();
    assert!(rbs.contains("def results: () -> ActiveRecord::Relation"), "{rbs}");
}
