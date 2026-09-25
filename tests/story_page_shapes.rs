//! Shapes lobsters' story page (`StoriesController#show`) reaches, each
//! lowered where Rails' meaning is decidable at the call site.
//!
//! - `request.format.html?` → `self.request_format == :html`: the
//!   controller's negotiated format, which main.rb seeds and every
//!   flattened `respond_to` reads. The CRuby overlay Request has no
//!   `format`, so the call fell to the private `Kernel#format`.
//! - `Vote.find_by(user: @user, story: @story, comment: nil)` → the
//!   foreign-key columns. Only `where` chains had the association keys
//!   translated; a bare class-level `find_by` rendered `WHERE user =
//!   '#<User…>'`.
//! - `@merged = [@story, @story.merged.not_deleted].flatten` threads
//!   into the view as `Array[Story]` — `flatten` splices a Relation's
//!   records, and a closure ivar whose NAME is no model now takes the
//!   analyzer's type — so `ms.comments.build` in the view lowers to
//!   `Comment.new(story_id: ms.id)` instead of calling `build` on an
//!   Array.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "username"
  end
  create_table "stories", force: :cascade do |t|
    t.integer "merged_story_id"
    t.boolean "is_deleted", default: false, null: false
    t.string "title"
  end
  create_table "comments", force: :cascade do |t|
    t.integer "story_id", null: false
    t.integer "user_id", null: false
  end
  create_table "votes", force: :cascade do |t|
    t.integer "user_id", null: false
    t.integer "story_id", null: false
    t.integer "comment_id"
  end
end
"#;

const CONTROLLER: &str = r#"class StoriesController < ApplicationController
  def show
    @story = Story.find(params[:id])
    @user = User.first
    return redirect_to("/") if request.format.html? && params[:title].to_s != @story.title
    @vote = Vote.find_by(user: @user, story: @story, comment: nil)
    @merged_stories = [@story, @story.merged_stories.not_deleted].flatten
  end
end
"#;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\n  has_many :comments\nend\n"),
        ("app/models/story.rb", "class Story < ApplicationRecord\n  has_many :comments\n  has_many :merged_stories, class_name: \"Story\", foreign_key: \"merged_story_id\"\n  scope :not_deleted, -> { where(is_deleted: false) }\nend\n"),
        ("app/models/comment.rb", "class Comment < ApplicationRecord\n  belongs_to :story\n  belongs_to :user\nend\n"),
        ("app/models/vote.rb", "class Vote < ApplicationRecord\n  belongs_to :user\n  belongs_to :story\n  belongs_to :comment, optional: true\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/stories_controller.rb", CONTROLLER),
        ("app/views/stories/show.html.erb", "<% @merged_stories.each do |ms| %><%= ms.comments.build.story_id %><% end %>\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"/s/:id/(:title)\" => \"stories#show\"\nend\n"),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn controller() -> String {
    ruby::emit_lowered_controllers(&app())
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("stories_controller.rb"))
        .map(|f| f.content)
        .expect("stories_controller.rb")
}

#[test]
fn request_format_predicate_reads_the_negotiated_format() {
    let out = controller();
    assert!(out.contains("self.request_format == :html"), "{out}");
    assert!(!out.contains("request.format"), "{out}");
}

#[test]
fn a_class_level_find_by_translates_association_keys() {
    let out = controller();
    assert!(
        out.contains("Vote.find_by({ user_id: @user, story_id: @story, comment_id: nil })"),
        "{out}"
    );
}

#[test]
fn a_flattened_ivar_types_the_view_loop_variable() {
    let views = ruby::emit_lowered_views(&app())
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!views.contains("ms.comments.build"), "{views}");
    assert!(views.contains("Comment.new"), "{views}");
}
