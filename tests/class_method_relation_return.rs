//! A model CLASS method whose body ends in a query chain keeps the
//! chain's model for what follows the call.
//!
//! Lobsters' comments controller:
//!
//! ```ruby
//! @threads = Comment.recent_threads(@showing_user)
//!   .merge(Story.not_deleted(@user))
//!   .for_presentation
//!   .joins(:story)
//! ```
//!
//! `recent_threads` ends in `Comment.joins(…).where(…).order(…)`. The
//! chain lowering tracked the model through scopes and instance methods
//! but not class methods, so `for_presentation` stayed a bare method
//! call and `joins(:story)` reached the runtime unresolved (a raise).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

#[test]
fn a_relation_returning_class_method_keeps_the_model() {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "stories", force: :cascade do |t|
    t.boolean "is_deleted", default: false, null: false
  end
  create_table "comments", force: :cascade do |t|
    t.integer "story_id", null: false
    t.integer "thread_id"
  end
end
"#,
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        (
            "app/models/story.rb",
            "class Story < ApplicationRecord\n  has_many :comments\n  scope :not_deleted, -> { where(is_deleted: false) }\nend\n",
        ),
        (
            "app/models/comment.rb",
            "class Comment < ApplicationRecord\n  belongs_to :story\n  scope :recent_first, -> { order(id: :desc) }\n\n  def self.recent_threads(ids)\n    Comment.where(thread_id: ids).order(\"comments.thread_id desc\")\n  end\n\n  def self.threads_for(ids)\n    Comment.recent_threads(ids).merge(Story.not_deleted).recent_first.joins(:story).to_a\n  end\nend\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("comment.rb"))
        .map(|f| f.content)
        .expect("comment.rb");
    assert!(
        src.contains("INNER JOIN stories ON stories.id = comments.story_id"),
        "the association join resolves:\n{src}"
    );
    assert!(src.contains("Comment.recent_first("), "the scope threads the relation:\n{src}");
}
