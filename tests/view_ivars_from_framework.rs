//! Three ways an ivar or a method goes missing from the ANALYZER while
//! being perfectly present in the emitted tree, all of them reported to
//! the user as `has no known type` / `no known method`.
//!
//! 1. **A framework method assigns the ivar.** geared_pagination's
//!    `set_page_and_extract_portion_from` sets `@page` inside
//!    `runtime/ruby/action_controller/pagination.rb`; the gem exposes no
//!    reader and the VIEW is the only consumer, so nothing syntactic in
//!    the action shows the assignment.
//! 2. **A catalog entry is missing.** `Model.where(…).new` builds a
//!    record carrying the scope's conditions. `build` (its alias) was
//!    catalogued and `new` was not — and the cost landed on the IVAR the
//!    result was assigned to, two templates away.
//! 3. **A lowering synthesizes the method.** `lower::attached` writes
//!    `def avatar; ActiveStorage::Attached.new(…); end` at the emit
//!    seam, after the analyzer has run. The method exists in the emitted
//!    tree and not in the analyzer's world.
//!
//! All three surfaced on campfire and all three are the same rule: a
//! name the pipeline knows about must be registered where the analyzer
//! can see it too.

use roundhouse::analyze::{diagnose, Analyzer};
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\n  \
    create_table :active_storage_blobs do |t|\n    t.string :key\n    \
    t.string :filename\n    t.string :content_type\n    t.integer :byte_size\n  end\n  \
    create_table :active_storage_attachments do |t|\n    t.string :name\n    \
    t.string :record_type\n    t.integer :record_id\n    t.integer :blob_id\n  end\nend\n";

const MODEL: &str = r#"
class User < ApplicationRecord
  has_one_attached :avatar
end
"#;

const CONTROLLER: &str = r#"
class UsersController < ApplicationController
  def index
    set_page_and_extract_portion_from User.all, per_page: 10
    @bot = User.all.new
  end
end
"#;

// Every read is in the VIEW, which is where all three failures showed up.
const VIEW: &str = "<p><%= @page.last? %></p>\n\
    <p><%= @page.next_param %></p>\n\
    <p><%= @bot.avatar.attached? %></p>\n\
    <p><%= @bot.avatar.filename %></p>\n";

fn diagnostics() -> Vec<String> {
    let tree = vec![
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :users\nend\n",
        ),
        ("app/models/user.rb", MODEL),
        ("app/controllers/users_controller.rb", CONTROLLER),
        ("app/views/users/index.html.erb", VIEW),
    ]
    .into_iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app).into_iter().map(|d| d.to_string()).collect()
}

fn offenders(needle: &str) -> Vec<String> {
    diagnostics()
        .into_iter()
        .filter(|d| d.starts_with("error") && d.contains(needle))
        .collect()
}

#[test]
fn page_is_bound_by_the_pagination_call_that_assigns_it() {
    let o = offenders("@page");
    assert!(o.is_empty(), "`set_page_and_extract_portion_from` binds @page: {o:?}");
}

#[test]
fn a_page_read_resolves_against_the_registered_class() {
    let o = offenders("last?");
    assert!(o.is_empty(), "ActionController::Page's surface must resolve: {o:?}");
}

#[test]
fn relation_new_answers_the_record() {
    let o = offenders("@bot");
    assert!(o.is_empty(), "`Model.all.new` answers a record, so @bot types: {o:?}");
}

#[test]
fn an_attachment_reader_resolves_and_so_does_what_it_answers() {
    let o = offenders("avatar");
    assert!(o.is_empty(), "has_one_attached's reader must be registered: {o:?}");
    let o = offenders("attached?");
    assert!(o.is_empty(), "ActiveStorage::Attached's surface must resolve: {o:?}");
}
