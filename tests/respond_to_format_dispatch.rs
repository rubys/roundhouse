//! Every REGISTERED mime format dispatches inside `respond_to`.
//!
//! The Collector's format names were a hand-kept list carrying
//! `html`/`json` and not `turbo_stream`, so `format.turbo_stream` read
//! as a dispatch failure in an app whose views and broadcasts handled
//! Turbo Streams fine (issue #74). The fixtures' `respond_to` blocks are
//! all html/json, and their Turbo Streams coverage is the broadcast and
//! view path, so no fixture ever crossed the two.
//!
//! The set is now derived from `runtime/ruby/mime.rb` — actionpack's own
//! registry, ported rather than retyped. `ics`/`pdf`/`vcf` are asserted
//! because they are registered formats the hand-kept list happened not
//! to carry, the same way it happened not to carry `turbo_stream`;
//! asserting through DISPATCH rather than against the registry map keeps
//! the test on the behavior an app would hit.

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
  def create
    respond_to do |format|
      format.html { redirect_to articles_path }
      format.turbo_stream
    end
  end

  def export
    respond_to do |format|
      format.html { redirect_to articles_path }
      format.ics
      format.pdf
      format.vcf
    end
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

fn no_dispatch_failures(app: &roundhouse::App) {
    let diags = diagnose(app);
    let failed: Vec<_> = diags
        .iter()
        .filter(|d| d.code() == "send_dispatch_failed")
        .collect();
    assert!(failed.is_empty(), "unexpected dispatch failures: {failed:?}");
}

#[test]
fn format_turbo_stream_resolves_in_respond_to() {
    let app = analyzed_app();
    let body = action_body(&app, "create");
    assert!(
        format!("{body:?}").contains("turbo_stream"),
        "the create action should carry the turbo_stream format call",
    );
    no_dispatch_failures(&app);
}

#[test]
fn formats_beyond_the_old_hand_kept_list_resolve() {
    let app = analyzed_app();
    let rendered = format!("{:?}", action_body(&app, "export"));
    for name in ["ics", "pdf", "vcf"] {
        assert!(rendered.contains(name), "export should call format.{name}");
    }
    no_dispatch_failures(&app);
}
