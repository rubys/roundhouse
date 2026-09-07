//! `group(:col).count` answers a Hash, and the checker has to say so.
//!
//! Rails' grouped count returns a Hash of group-key => COUNT.
//! `lower::group_count` already renames the terminal and
//! `ActiveRecord::Relation#group_count` already builds the
//! `SELECT … GROUP BY` and hydrates the Hash — but that lowering runs on
//! the POST-ANALYZE hook, so the typer saw the source spelling `count`,
//! answered `Int` from the catalog, and `.keys` on the result read as
//! `no known method keys on Int` (issue #75). A fully implemented
//! feature was unreachable through the checker because nothing in
//! `fixtures/` ever grouped a count.
//!
//! The assertions are deliberately paired: the TYPE the analyzer reports
//! and the METHOD the lowering emits are checked together, because the
//! bug was the two disagreeing.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::expr::ExprNode;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const FILES: &[(&str, &str)] = &[
    (
        "db/schema.rb",
        r#"ActiveRecord::Schema.define do
  create_table "feeds", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.integer "feed_id", null: false
    t.boolean "read", null: false
  end
end
"#,
    ),
    (
        "app/models/feed.rb",
        "class Feed < ApplicationRecord\n  has_many :articles\nend\n",
    ),
    (
        "app/models/article.rb",
        r#"class Article < ApplicationRecord
  belongs_to :feed
  scope :unread, -> { where(read: false) }
end
"#,
    ),
    (
        "app/controllers/application_controller.rb",
        "class ApplicationController < ActionController::Base\nend\n",
    ),
    (
        "app/controllers/articles_controller.rb",
        r#"class ArticlesController < ApplicationController
  def index
    @by_feed = Article.where(read: false).group(:feed_id).count
    @feed_ids = @by_feed.keys
    @scoped = Article.unread.group(:feed_id).count
    @plain = Article.where(read: false).count
  end
end
"#,
    ),
    (
        "config/routes.rb",
        "Rails.application.routes.draw do\n  resources :articles\nend\n",
    ),
];

fn tree() -> HashMap<PathBuf, Vec<u8>> {
    FILES
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// Analyze only — the state the checker reports on, before any lowering
/// has had a chance to rewrite the source spelling.
fn analyzed_app() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree()).expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    app
}

fn action_body(app: &roundhouse::App, name: &str) -> roundhouse::expr::Expr {
    app.controllers
        .iter()
        .flat_map(|c| c.actions())
        .find(|a| a.name.as_str() == name)
        .unwrap_or_else(|| panic!("no `{name}` action"))
        .body
        .clone()
}

/// The value type of `index`'s assignment to `@name`.
fn ivar_ty(app: &roundhouse::App, name: &str) -> Ty {
    let ExprNode::Seq { exprs } = &*action_body(app, "index").node else {
        panic!("expected Seq body");
    };
    for e in exprs {
        if let ExprNode::Assign { target, value } = &*e.node {
            if format!("{target:?}").contains(name) {
                return value
                    .ty
                    .clone()
                    .unwrap_or_else(|| panic!("no ty on the @{name} assignment"));
            }
        }
    }
    panic!("no assignment to @{name}");
}

fn hash_of_int_keys() -> Ty {
    Ty::Hash { key: Box::new(Ty::Int), value: Box::new(Ty::Int) }
}

#[test]
fn grouped_count_types_as_a_hash_keyed_by_the_grouped_column() {
    let app = analyzed_app();
    // `feed_id` is an integer column, so `.keys` is `Array[Int]` — the
    // schema answers the key type, the way `pluck(:col)` is answered.
    // `Hash[Untyped, Int]` would type `.keys` and hand every strict
    // target an untyped element.
    assert_eq!(ivar_ty(&app, "by_feed"), hash_of_int_keys());
}

#[test]
fn grouped_count_types_the_same_through_a_scope_receiver() {
    // `Article.unread` types as `Ty::Relation`, the inline `where` chain
    // as the Array representation. Both are relations that got grouped,
    // and neither carries the group in its TYPE — the receiver
    // EXPRESSION is the only discriminator, so both must be read.
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "scoped"), hash_of_int_keys());
}

#[test]
fn ungrouped_count_is_still_an_integer() {
    // The scalar count must not move. A `count` answering
    // Integer-or-Hash is the polymorphic return the name split exists to
    // avoid.
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "plain"), Ty::Int);
}

#[test]
fn keys_on_a_grouped_count_dispatches() {
    let app = analyzed_app();
    assert_eq!(ivar_ty(&app, "feed_ids"), Ty::Array { elem: Box::new(Ty::Int) });
    let diags = diagnose(&app);
    let failed: Vec<_> = diags
        .iter()
        .filter(|d| d.code() == "send_dispatch_failed")
        .collect();
    assert!(failed.is_empty(), "unexpected dispatch failures: {failed:?}");
}

#[test]
fn the_typed_terminal_is_the_one_the_lowering_emits() {
    // The analyzer's shape test and `lower::group_count`'s must agree:
    // typing a `count` as a Hash the lowering does NOT rename would
    // report a type no runtime method answers. Run the full pipeline and
    // confirm both grouped terminals became `group_count`.
    let mut app = ingest_app_from_tree(tree()).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    fn walk(e: &roundhouse::expr::Expr, found: &mut usize) {
        if let ExprNode::Send { method, .. } = &*e.node {
            if method.as_str() == "group_count" {
                *found += 1;
            }
        }
        e.node.for_each_child(&mut |c| walk(c, found));
    }
    let mut found = 0;
    for c in &app.controllers {
        for a in c.actions() {
            walk(&a.body, &mut found);
        }
    }
    assert_eq!(found, 2, "both grouped counts should lower to `group_count`");
}
