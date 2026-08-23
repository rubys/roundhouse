//! `<relation>.new(…)` is rewritten to the MODEL's constructor
//! (`scope_chain::rewrite_send`).
//!
//! Rails builds a record through a relation — `User.active_bots.new`,
//! `room.memberships.new(attrs)` — and seeds it from the relation's
//! create-scope. There is no `ActiveRecord::Relation#new` in this
//! runtime and there cannot be one: under spinel a class's constructor
//! is already `sp_Relation_new`, so an instance method of that name is
//! a duplicate C symbol and the whole program stops compiling. One
//! landed briefly and turned every spinel job red; relation.rb keeps
//! the hole with a note, and docs/pipeline/runtime.md ledgers it.
//!
//! So the call moves to the call site. The receiver stays where it is —
//! a relation is lazy, so reading `scope_attributes` off it runs no
//! query — and the caller's own attributes ride on the OUTSIDE of the
//! merge, because Rails assigns them after the scope's.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
    t.string "role", null: false
    t.boolean "active", null: false
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  scope :active_bots, -> { where(active: true) }
end
"#,
        ),
        (
            "app/controllers/bots_controller.rb",
            r#"class BotsController < ApplicationController
  def new
    @bot = User.active_bots.new
  end

  def build
    @bot = User.active_bots.new(name: params[:name])
  end
end
"#,
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn shown() -> String {
    ruby::emit_lowered_controllers(&app())
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/controllers/bots_controller.rb"))
        .map(|f| f.content)
        .expect("bots_controller emitted")
}

/// The bare form: the model constructs, carrying the relation's scope.
#[test]
fn a_bare_relation_new_constructs_the_model_with_the_scope_attributes() {
    let src = shown();
    assert!(
        src.contains("User.new(User.active_bots.scope_attributes)"),
        "User.active_bots.new builds a User seeded from the scope:\n{src}"
    );
    assert!(
        !src.contains("active_bots.new"),
        "no `new` may survive on a Relation receiver — the name is spoken for:\n{src}"
    );
}

/// …and the caller's own attributes stay on the OUTSIDE of the merge,
/// which is Rails' order: an explicit value wins over the scope's.
#[test]
fn the_callers_own_attributes_win_over_the_scopes() {
    let src = shown();
    assert!(
        src.contains(".scope_attributes.merge("),
        "explicit attributes merge OVER the scope's:\n{src}"
    );
    let merge = src.split(".scope_attributes.merge(").nth(1).unwrap_or("");
    assert!(
        merge.starts_with("name:"),
        "the caller's hash is the merge ARGUMENT, not its receiver:\n{src}"
    );
}
