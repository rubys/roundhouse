//! A model CLASS METHOD reached through an association takes that
//! association's relation as its scope (`scope_chain::AssocClassMethods`).
//!
//! Rails runs `user.sessions.start!(…)` and `@room.notes.paged?` with
//! the association as the current scope. The method gains a trailing
//! `__rel`, the call site passes the seed, and the body reads it in one
//! of two ways:
//!
//!   * a CONSTRUCTOR merges `__rel.scope_attributes` UNDER its own
//!     attributes (Rails assigns explicit ones after the scope's, so an
//!     explicit value wins) — which is how `scope_for_create` gets
//!     `user_id` onto the row without anybody naming the column;
//!   * a QUERY roots on `__rel` exactly as a scope body does — so
//!     `paged?`'s bare `count` counts THIS room's notes.
//!
//! Our association reader is arel-folded to an Array, so neither call
//! resolves at all today.
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
  PAGE_SIZE = 40

  scope :ordered, -> { order(:id) }
  scope :earlier, ->(note) { where("id < ?", note.id) }
  scope :later, ->(note) { where("id > ?", note.id) }

  def self.file!(attributes)
    create!(attributes)
  end

  def self.paged?
    count > PAGE_SIZE
  end

  def self.page_around(note)
    earlier(note) + [ note ] + later(note)
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
    @paged = @room.notes.paged?
    notes = @room.notes.ordered
    @around = notes.page_around(@note)
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
             .preloaded(Current.user.memberships_target, Current.user.memberships_loaded?)\
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
        note.contains("def self.file!(attributes)"),
        "a blocked method grows no relation parameter — unlike its \
         admitted siblings on the same model:\n{note}"
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

// ---- the QUERY half -------------------------------------------------

/// A class method whose body READS rather than constructs takes the
/// same relation and roots its query on it. `Note.paged?`'s bare
/// `count` is the whole point: Rails runs it against the caller's
/// scope, so through `@room.notes` it counts THIS room's notes.
#[test]
fn a_query_shaped_class_method_roots_its_terminal_on_the_relation() {
    let note = model("app/models/note.rb");
    assert!(
        note.contains("def self.paged?(__rel = ActiveRecord::Relation.new(self))"),
        "the query-shaped method grows the same trailing parameter:\n{note}"
    );
    assert!(
        note.contains("__rel.count >"),
        "and its bare terminal roots on the threaded relation:\n{note}"
    );
}

/// The bare `count` survives ingest for exactly this method.
/// `qualify_model_class_method_ar_calls` names the model on a class
/// method's implicit-self AR calls, which would spell the scoped form
/// (`count`) as the deliberately-unscoped one (`Note.count`) past the
/// point where anything could tell them apart — so a method reached
/// through an association is left alone. Without that, this method
/// would emit an inlined whole-table `SELECT COUNT(*)`.
#[test]
fn the_scoped_count_is_not_inlined_as_a_whole_table_query() {
    let note = model("app/models/note.rb");
    assert!(
        !note.contains("def self.paged?(__rel = ActiveRecord::Relation.new(self))\n    stmt ="),
        "the body is the relation read, not an arel-folded table count:\n{note}"
    );
    let create = controller();
    assert!(
        create.contains(
            "Note.paged?(ActiveRecord::Relation.new(Note).where_scope(room_id: @room.id))"
        ),
        "and the call site passes the association's relation:\n{create}"
    );
}

/// Both scope calls take the relation, including the one in ARGUMENT
/// position. An argument is evaluated with the same `self` the receiver
/// was, so Rails scopes both halves of `earlier(n) + [n] + later(n)`;
/// threading only the receiver would leave the second running against
/// the whole table.
#[test]
fn a_scope_call_in_argument_position_is_scoped_too() {
    let note = model("app/models/note.rb");
    assert!(
        note.contains("def self.page_around(note, __rel = ActiveRecord::Relation.new(self))"),
        "page_around grows the parameter:\n{note}"
    );
    assert!(
        note.contains("Note.earlier(note, __rel) + [ note ] + Note.later(note, __rel)"),
        "both scope calls thread it, receiver AND argument:\n{note}"
    );
}

/// The relation is often already in hand — `notes = @room.notes
/// .ordered` — and then it is threaded as-is rather than re-seeded.
/// Unlike a scope the method returns whatever its body returns (an
/// Array here), so the chain does not continue through it.
#[test]
fn a_relation_already_in_a_local_is_threaded_without_reseeding() {
    let create = controller();
    assert!(
        create.contains("Note.page_around(@note, notes)"),
        "the local relation is passed straight through:\n{create}"
    );
}
