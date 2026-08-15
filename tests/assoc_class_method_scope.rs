//! A model CLASS METHOD reached through an association takes that
//! association's relation as its create scope
//! (`scope_chain::AssocClassMethods`).
//!
//! Rails runs `user.sessions.start!(…)` with the association as the
//! current scope, so the `create!` inside `Session.start!` picks the
//! foreign key up from it (`scope_for_create`) and the row lands owned
//! by that user without anybody naming `user_id`. Our association reader
//! is arel-folded to an Array, so the call does not even resolve — and
//! the fix is the mechanism a scope already uses one layer over: the
//! method gains a trailing `__rel`, the call site passes the seed, and
//! the body's constructor merges `__rel.scope_attributes` UNDER its own
//! attributes (Rails lets an explicit attribute win).
//!
//! Demand-gated: only a method some call site actually reaches through
//! an association grows the parameter, so an app that never writes the
//! shape emits exactly what it emitted before.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "sessions", force: :cascade do |t|
    t.integer "user_id", null: false
    t.string "user_agent"
    t.string "ip_address"
  end
  create_table "memberships", force: :cascade do |t|
    t.integer "user_id", null: false
    t.integer "room_id", null: false
  end
  create_table "notes", force: :cascade do |t|
    t.integer "room_id", null: false
    t.string "body", null: false
  end
end
"#;

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  has_many :sessions
  has_many :memberships
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  has_many :notes
end
"#,
        ),
        (
            "app/models/session.rb",
            r#"class Session < ApplicationRecord
  scope :recent, -> { order(:id) }

  def self.start!(user_agent:, ip_address:)
    create! user_agent: user_agent, ip_address: ip_address
  end
end
"#,
        ),
        (
            "app/models/membership.rb",
            r#"class Membership < ApplicationRecord
end
"#,
        ),
        (
            "app/models/note.rb",
            r#"class Note < ApplicationRecord
  def self.file!(attributes)
    create!(attributes)
  end
end
"#,
        ),
        (
            "app/controllers/sessions_controller.rb",
            r#"class SessionsController < ApplicationController
  def create
    user = User.find(params[:user_id])
    @session = user.sessions.start!(user_agent: "x", ip_address: "y")
    @direct = Session.start!(user_agent: "x", ip_address: "y")
    @membership = Current.user.memberships.find_by!(room_id: params[:room_id])
    @room = Room.find(params[:id])
    @note = @room.notes.file!(note_params)
  end
end
"#,
        ),
    ]))
    .expect("ingest")
}

fn emitted(files: &[roundhouse::emit::EmittedFile], suffix: &str) -> String {
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

fn controller() -> String {
    emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/sessions_controller.rb",
    )
}

fn model(name: &str) -> String {
    emitted(&ruby::emit_lowered_models(&app()), name)
}

/// The call site: a local owner (`user`) resolves through its NAME, and
/// the seed is spelled `where_scope` — the same filter a plain seed
/// applies, plus the record of what a create through it presets.
#[test]
fn class_method_through_an_association_takes_the_seeded_relation() {
    let create = controller();
    assert!(
        create.contains(
            "Session.start!(ActiveRecord::Relation.new(Session).where_scope(user_id: user.id), \
             user_agent: \"x\", ip_address: \"y\")"
        ),
        "user.sessions.start! threads the association's relation:\n{create}"
    );
}

/// The method side: the parameter lands before the keywords (as
/// `push_scope_methods` places it), and the constructor merges the
/// scope attributes UNDER the caller's own — Rails assigns explicit
/// attributes after the scope's, so an explicit value wins.
#[test]
fn the_class_method_grows_a_rel_param_and_merges_its_scope_attributes() {
    let session = model("app/models/session.rb");
    assert!(
        session.contains(
            "def self.start!(__rel = ActiveRecord::Relation.new(self), user_agent:, ip_address:)"
        ),
        "__rel is inserted before the keywords:\n{session}"
    );
    assert!(
        session.contains(
            "create! __rel.scope_attributes.merge(user_agent: user_agent, ip_address: ip_address)"
        ),
        "the constructor merges the scope under its own attributes:\n{session}"
    );
}

/// The same method called on the CLASS is untouched: the default
/// `Relation.new(self)` carries no scope attributes, so the merge is
/// with an empty hash and the row is written exactly as before.
#[test]
fn a_direct_class_call_still_binds_and_scopes_nothing() {
    let create = controller();
    assert!(
        create.contains("Session.start!(user_agent: \"x\", ip_address: \"y\")"),
        "the bare class call is left alone:\n{create}"
    );
}

/// `find_by!` through an association is a QUERY, not an Array scan —
/// and the owner here is a one-hop read (`Current.user`), the form
/// every room-scoped controller opens with.
#[test]
fn find_by_bang_through_a_read_owner_seeds_a_relation() {
    let create = controller();
    assert!(
        create.contains(
            "ActiveRecord::Relation.new(Membership).where(user_id: Current.user.id)\
             .find_by!(room_id:"
        ),
        "Current.user.memberships.find_by! seeds a relation:\n{create}"
    );
}

/// A constructor whose argument is not an attribute hash cannot take
/// the merge — the value's own shape is unknown here, and emitting
/// `attributes.merge(...)` against something that may not be a Hash
/// trades a loud failure for a wrong one. The method declines whole:
/// no `__rel`, and the call site keeps its source shape.
#[test]
fn an_opaque_constructor_argument_declines_rather_than_guessing() {
    let note = model("app/models/note.rb");
    assert!(
        !note.contains("__rel"),
        "a blocked method grows no relation parameter:\n{note}"
    );
    assert!(
        note.contains("create!(attributes)"),
        "its constructor is untouched:\n{note}"
    );
    let create = controller();
    assert!(
        create.contains("@room.notes.file!("),
        "and the call site keeps its source shape:\n{create}"
    );
}
