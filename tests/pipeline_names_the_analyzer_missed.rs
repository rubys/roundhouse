//! Names the emitted tree resolves and the ANALYZER did not, plus the
//! two gates that a better type must not move.
//!
//! * an association EXTENSION method (`room.memberships.grant_to(u)`),
//!   which belongs to neither class involved and cannot be found from
//!   the receiver's type at all — the association read is
//!   `Array<Membership>` and has forgotten which association made it;
//! * `record.becomes!(Rooms::Closed)`, whose answer is named by the
//!   ARGUMENT;
//! * `<col>_previously_was`, the one member of the ActiveModel::Dirty
//!   family the registry did not name, though `model_to_library` has
//!   been synthesizing it (with a hydration baseline) all along;
//! * Turbo::Broadcastable's four `broadcast_*_to`, which turbo-rails
//!   mixes into every model and `lower::…::broadcasts` rewrites at
//!   every call site, controller bodies included.
//!
//! And the gate: `select { … }` is Enumerable's filter, `select(:col)`
//! is the projection, and `lower::relation_select_block` sends the
//! block form to the name that means it. Its gate read "Relation, or a
//! type I don't know" — so the SAME call stopped being rewritten the
//! day its receiver got a type, and raised `select: no columns`. A type
//! getting better must not change what a gate decides about one value.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :users do |t|\n    t.string :name\n  end\n  \
    create_table :rooms do |t|\n    t.string :name\n    t.string :type\n  end\n  \
    create_table :memberships do |t|\n    t.integer :user_id\n    t.integer :room_id\n    \
    t.string :involvement\n  end\nend\n";

const ROOM: &str = r#"
class Room < ApplicationRecord
  has_many :memberships do
    def grant_to(users)
      users
    end
  end
end
"#;

const CLOSED: &str = "class Rooms::Closed < Room\nend\n";
const USER: &str = "class User < ApplicationRecord\n  has_many :memberships\nend\n";
const MEMBERSHIP: &str =
    "class Membership < ApplicationRecord\n  belongs_to :user\n  belongs_to :room\nend\n";

fn app_with(action_body: &str) -> roundhouse::App {
    let controller = format!(
        "class RoomsController < ApplicationController\n  def index\n    {action_body}\n  end\nend\n"
    );
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA.to_string()),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n".to_string(),
        ),
        ("app/models/room.rb", ROOM.to_string()),
        ("app/models/rooms/closed.rb", CLOSED.to_string()),
        ("app/models/user.rb", USER.to_string()),
        ("app/models/membership.rb", MEMBERSHIP.to_string()),
        ("app/controllers/rooms_controller.rb", controller),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn errors(action_body: &str, needle: &str) -> Vec<String> {
    diagnose(&app_with(action_body))
        .into_iter()
        .map(|d| d.to_string())
        .filter(|d| d.starts_with("error") && d.contains(needle))
        .collect()
}

fn emitted(action_body: &str) -> String {
    ruby::emit_spinel(&app_with(action_body))
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("rooms_controller.rb"))
        .map(|f| f.content)
        .expect("rooms_controller emitted")
}

#[test]
fn an_association_extension_method_resolves() {
    let o = errors("Room.first.memberships.grant_to([])", "grant_to");
    assert!(o.is_empty(), "the extension block declares it: {o:?}");
}

/// The negative twin: the pair has to be REGISTERED, so this stays
/// dispatch resolution rather than method_missing over an Array.
#[test]
fn an_undeclared_name_on_the_same_association_still_lands() {
    let o = errors("Room.first.memberships.grant_to_nobody([])", "grant_to_nobody");
    assert_eq!(o.len(), 1, "an undeclared extension name is still a gap: {o:?}");
}

/// An STI subclass declares no associations of its own, so the lookup
/// has to walk to the base — which is exactly the receiver campfire has
/// after `becomes!`.
#[test]
fn the_extension_is_reachable_through_an_sti_subclass() {
    let o = errors(
        "Room.first.becomes!(Rooms::Closed).memberships.grant_to([])",
        "grant_to",
    );
    assert!(o.is_empty(), "walk the parent chain for the association: {o:?}");
}

#[test]
fn becomes_answers_the_class_it_names() {
    let o = errors("@room = Room.first.becomes!(Rooms::Closed)\n    @room.name", "name");
    assert!(o.is_empty(), "the ARGUMENT names the answer: {o:?}");
}

#[test]
fn the_after_commit_dirty_reader_is_registered() {
    let o = errors("Membership.first.involvement_previously_was", "previously_was");
    assert!(
        o.is_empty(),
        "`model_to_library::schema` synthesizes it, so the analyzer must know it: {o:?}"
    );
}

#[test]
fn turbo_broadcasts_resolve_on_a_model_instance() {
    for m in [
        "broadcast_append_to",
        "broadcast_prepend_to",
        "broadcast_replace_to",
        "broadcast_remove_to",
    ] {
        let o = errors(&format!("Room.first.{m} Room.first, :rooms"), m);
        assert!(o.is_empty(), "turbo-rails mixes {m} into every model: {o:?}");
    }
}

/// The gate that must not move when a type improves: an association
/// read is `Array<Model>` in this type system and an
/// `ActiveRecord::Relation` at runtime, where `select` is the
/// projection. The block form has to reach `filter` either way.
#[test]
fn a_block_select_on_an_association_read_becomes_filter() {
    let src = emitted("@rooms = Room.first.memberships.select { |m| m.involvement == \"x\" }");
    assert!(
        src.contains(".filter {") && !src.contains(".select {"),
        "the block form is Enumerable's, not the projection:\n{src}"
    );
}
