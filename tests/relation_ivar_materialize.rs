//! `lower::relation_ivar_materialize`: a controller ivar that is an
//! Array on one branch and a Relation on another has the Relation
//! branch loaded at the assignment, so the view parameter the ivar
//! feeds — declared `Array[Model]` from its name — receives the shape
//! it declares on every path. campfire's searches controller is the
//! shape: `.last(100)` in one branch, `Message.none` in the other.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::Analyzer;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::apply_relation_ivar_materialize;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn controller_src(controller_body: &str) -> String {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.text "body", null: false
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  scope :ordered, -> { order(:id) }
end
"#,
        ),
        ("app/controllers/searches_controller.rb", controller_body),
        (
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "/searches", to: "searches#index", as: :searches
  get "/searches/all", to: "searches#all", as: :all_searches
end
"#,
        ),
        (
            "app/views/searches/index.html.erb",
            "<% @messages.each do |message| %><%= message.body %><% end %>\n",
        ),
        (
            "app/views/searches/all.html.erb",
            "<% @messages.each do |message| %><%= message.body %><% end %>\n",
        ),
    ]))
    .expect("ingest");
    Analyzer::new(&app).analyze(&mut app);
    apply_relation_ivar_materialize(&mut app);
    let files = ruby::emit_lowered_controllers(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("searches_controller.rb"))
        .map(|f| f.content.clone())
        .expect("searches_controller.rb")
}

#[test]
fn relation_branch_of_a_mixed_ivar_is_loaded_at_the_assignment() {
    let src = controller_src(
        r#"class SearchesController < ApplicationController
  def index
    if params[:q].present?
      @messages = Message.ordered.last(100)
    else
      @messages = Message.none
    end
  end
end
"#,
    );
    assert!(
        src.contains(".none.to_a"),
        "the Relation branch must be materialised:\n{src}"
    );
    assert!(
        !src.contains("last_n(100).to_a") && !src.contains("last(100).to_a"),
        "the Array branch is left alone:\n{src}"
    );
}

#[test]
fn an_ivar_that_is_only_ever_a_relation_is_left_alone() {
    // The working contract for a Relation-only ivar is the call site's:
    // nothing to reconcile, so nothing is loaded early.
    let src = controller_src(
        r#"class SearchesController < ApplicationController
  def index
    @messages = Message.ordered
  end

  def all
    @messages = Message.none
  end
end
"#,
    );
    assert!(!src.contains(".to_a"), "no materialisation expected:\n{src}");
}
