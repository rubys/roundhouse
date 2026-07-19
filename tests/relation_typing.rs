//! `Ty::Relation` producer + dispatch coverage (relation-type-plan
//! R3/R4): scope calls and relation-returning class methods type as
//! `Relation { of }`, chains preserve the representation, terminals
//! produce exactly the types the Array representation produces, and
//! class-side delegation resolves scopes on relation receivers.
//!
//! Inline MapVfs app so the fixture stays next to the assertions —
//! tiny-blog/real-blog deliberately keep their emit-stable shapes.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;
use roundhouse::{ClassId, Symbol};

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// One app exercising every producer/consumer pair under test.
fn analyzed_app() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
    t.integer "score", null: false
    t.integer "user_id", null: false
  end
  create_table "comments", force: :cascade do |t|
    t.text "body", null: false
    t.integer "story_id", null: false
  end
end
"#,
        ),
        (
            "app/models/comment.rb",
            r#"class Comment < ApplicationRecord
  belongs_to :story
end
"#,
        ),
        (
            "app/models/story.rb",
            r#"class Story < ApplicationRecord
  has_many :comments
  scope :recent, -> { order(score: :desc).limit(10) }
  scope :top, -> { recent.where("score > 0") }
  scope :titles_only, -> { pluck(:title) }

  def self.for_user(user_id)
    where(user_id: user_id)
  end

  def self.best_title
    order(score: :desc).first
  end
end
"#,
        ),
        (
            "app/controllers/stories_controller.rb",
            r#"class StoriesController < ApplicationController
  def index
    @relation = Story.recent
    @chained = Story.recent.where(user_id: 1)
    @delegated = Story.recent.for_user(1)
    @first = Story.recent.first
    @count = Story.recent.count
    @list = Story.recent.to_a
    @classm = Story.for_user(1)
    @scope_on_scope = Story.top
    @terminal_scope = Story.titles_only
  end

  def build_probe
    story = Story.find(params[:id])
    @built = story.comments.build
  end
end
"#,
        ),
        (
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "/stories", to: "stories#index", as: :stories
end
"#,
        ),
        (
            "app/views/stories/index.html.erb",
            "<% @list.each do |story| %><%= story.title %><% end %>\n",
        ),
    ]))
    .expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

/// `index`'s body is a Seq of ivar assigns; return the value type of
/// the assignment to `@name`.
fn ivar_ty(app: &roundhouse::App, name: &str) -> Ty {
    let index = app.controllers[0]
        .actions()
        .find(|a| a.name.as_str() == "index")
        .expect("index action");
    let ExprNode::Seq { exprs } = &*index.body.node else {
        panic!("expected Seq body");
    };
    for e in exprs {
        if let ExprNode::Assign { target, value } = &*e.node {
            if format!("{target:?}").contains(name) {
                return value.ty.clone().unwrap_or_else(|| {
                    panic!("no ty on the @{name} assignment value")
                });
            }
        }
    }
    panic!("no assignment to @{name} found");
}

fn story() -> ClassId {
    ClassId(Symbol::from("Story"))
}

fn relation_of_story() -> Ty {
    Ty::Relation { of: story() }
}

#[test]
fn scope_call_types_as_relation() {
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "relation"), relation_of_story());
}

#[test]
fn builder_chain_preserves_relation() {
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "chained"), relation_of_story());
}

#[test]
fn class_side_scope_delegates_on_relation_receiver() {
    // `Story.recent.for_user(1)` — `for_user` is a class method whose
    // body tail is a builder chain; it must resolve on the relation
    // receiver and keep the relation representation.
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "delegated"), relation_of_story());
}

#[test]
fn relation_terminals_produce_array_representation_types() {
    // Settled decision: terminal result types must not change.
    let app = analyzed_app();
    assert_eq!(
        ivar_ty(&app, "first"),
        Ty::Union {
            variants: vec![Ty::Class { id: story(), args: vec![] }, Ty::Nil]
        },
    );
    assert_eq!(ivar_ty(&app, "count"), Ty::Int);
    assert_eq!(
        ivar_ty(&app, "list"),
        Ty::Array { elem: Box::new(Ty::Class { id: story(), args: vec![] }) },
    );
}

#[test]
fn relation_returning_class_method_types_as_relation() {
    // `Story.for_user(1)` — a class method whose body tail is a
    // builder chain declares `Relation { of: Story }` to callers.
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "classm"), relation_of_story());
}

#[test]
fn scope_rooted_at_sibling_scope_types_as_relation() {
    // `scope :top, -> { recent.where(...) }` — a scope rooted at a
    // sibling scope call still seeds the relation type.
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "scope_on_scope"), relation_of_story());
}

#[test]
fn terminal_tailed_scope_keeps_legacy_array_seed() {
    // Conservative NON-flip: `scope :titles_only, -> { pluck(:title) }`
    // ends in a terminal, so the classifier must refuse the Relation
    // seed and the legacy `Array<Self>` stand-in stays.
    let app = analyzed_app();
    assert_eq!(
        ivar_ty(&app, "terminal_scope"),
        Ty::Array { elem: Box::new(Ty::Class { id: story(), args: vec![] }) },
    );
}

#[test]
fn assoc_collection_build_rewrites_to_fk_preset_constructor() {
    // Ruby-lane lowering (relation-type-plan R5): `story.comments
    // .build` on a TYPED owner rewrites to the target constructor
    // with the association FK preset — `Comment.new(story_id:
    // story.id)` — instead of calling `build` on the folded Array
    // reader. Owner typing is what disambiguates assoc names
    // declared on several models.
    let app = analyzed_app();
    let controllers = roundhouse::emit::ruby::emit_lowered_controllers(&app);
    let src: String = controllers
        .iter()
        .map(|f| f.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        src.contains("Comment.new(story_id: story.id)"),
        "expected FK-preset constructor rewrite in emitted controller:\n{src}",
    );
}

#[test]
fn view_iteration_over_materialized_list_still_types_element() {
    // `@list` is `Array<Story>` (to_a terminal); the view's
    // `@list.each { |story| story.title }` must type `story.title`
    // as the column's String — proves block-param typing survives
    // the relation-typed chain upstream.
    let app = analyzed_app();
    let view = app
        .views
        .iter()
        .find(|v| v.name.as_str().contains("index"))
        .expect("index view");
    let mut found_title_str = false;
    fn walk(e: &roundhouse::expr::Expr, found: &mut bool) {
        if let ExprNode::Send { method, recv, .. } = &*e.node {
            if method.as_str() == "title"
                && matches!(
                    recv.as_ref().and_then(|r| r.ty.as_ref()),
                    Some(Ty::Class { id, .. }) if id.0.as_str() == "Story"
                )
                && e.ty == Some(Ty::Str)
            {
                *found = true;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    walk(&view.body, &mut found_title_str);
    assert!(found_title_str, "story.title should type as Str in the view");
}
