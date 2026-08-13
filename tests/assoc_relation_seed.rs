//! Seeding a Relation for a scope called on a has_many read
//! (`scope_chain::assoc_owner_seed`).
//!
//! The association reader is arel-folded to an eager query returning an
//! Array, so relation surface following the read — `@room.messages
//! .with_creator` — has to be seeded from the association's foreign key
//! instead (`Relation.new(Message).where(room_id: @room.id)`).
//!
//! Resolving WHICH association takes three rungs: the owner's stamped
//! type, the owner's NAME, then the assoc name when it is unique across
//! all models. The middle rung is what carries real apps — two models
//! declaring the same collection is ordinary Rails (campfire has
//! `has_many :messages` on both Room and User), and that collision makes
//! the by-name rung answer None for every one of them.
//!
//! The seed is exactly `where(fk => owner.id)`, so any declaration it
//! cannot reproduce must DECLINE rather than answer with wrong rows.

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
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
    t.integer "room_id", null: false
    t.integer "creator_id", null: false
  end
  create_table "notes", force: :cascade do |t|
    t.string "text", null: false
    t.integer "notable_id", null: false
    t.string "notable_type", null: false
  end
  create_table "tags", force: :cascade do |t|
    t.string "tag", null: false
    t.integer "room_id", null: false
    t.integer "recent_room_id", null: false
  end
end
"#;

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  has_many :messages
  has_many :notes, as: :notable
  has_many :tags, -> { where(archived: false) }
  has_many :recent_tags, -> { includes :room }, class_name: "Tag", foreign_key: :recent_room_id
end
"#,
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  has_many :messages, foreign_key: :creator_id
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  scope :with_creator, -> { order(:created_at) }
end
"#,
        ),
        (
            "app/models/note.rb",
            r#"class Note < ApplicationRecord
  scope :recent, -> { order(:id) }
end
"#,
        ),
        (
            "app/models/tag.rb",
            r#"class Tag < ApplicationRecord
  scope :alphabetical, -> { order(:tag) }
end
"#,
        ),
        (
            "app/controllers/rooms_controller.rb",
            r#"class RoomsController < ApplicationController
  def show
    @room = Room.find(params[:id])
    @user = User.find(params[:user_id])
    @messages = @room.messages.with_creator
    @mine = @user.messages.with_creator
    @notes = @room.notes.recent
    @tags = @room.tags.alphabetical
    @recent = @room.recent_tags.alphabetical
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

fn shown() -> String {
    emitted(&ruby::emit_lowered_controllers(&app()), "app/controllers/rooms_controller.rb")
}

/// `messages` is declared on BOTH Room and User, so the by-assoc-name
/// rung answers None. The owner ivar's own name resolves each one to the
/// right declaration — and therefore to the right foreign key.
#[test]
fn ambiguous_assoc_name_resolves_through_the_owner_ivar_name() {
    let show = shown();
    assert!(
        show.contains("Message.with_creator(ActiveRecord::Relation.new(Message).where(room_id: @room.id))"),
        "@room.messages seeds room_id:\n{show}"
    );
    assert!(
        show.contains("Message.with_creator(ActiveRecord::Relation.new(Message).where(creator_id: @user.id))"),
        "@user.messages seeds creator_id, not room_id:\n{show}"
    );
}

/// `as: :notable` keys rows by `notable_id` AND `notable_type`. A seed
/// of only the id half would reach every other implementor's rows, so
/// the chain is declined and keeps its source shape.
#[test]
fn polymorphic_has_many_declines_rather_than_seeding_half_the_key() {
    let show = shown();
    assert!(
        !show.contains("Relation.new(Note).where(notable_id:"),
        "no id-only seed for an `as:` association:\n{show}"
    );
    assert!(show.contains("@room.notes.recent"), "chain keeps its source shape:\n{show}");
}

/// A scope that can change the row set (`-> { where(archived: false) }`)
/// is not reproduced by the bare FK seed, so it declines too.
#[test]
fn row_changing_association_scope_declines() {
    let show = shown();
    assert!(
        !show.contains("Relation.new(Tag).where(room_id: @room.id))"),
        "no unscoped seed for a where-scoped association:\n{show}"
    );
    assert!(show.contains("@room.tags.alphabetical"), "chain keeps its source shape:\n{show}");
}

/// An eager-load-only scope names what to load alongside, never which
/// rows to return — so the seed still fires. Declining it would turn a
/// slow chain into a broken one (lobsters'
/// `author.stories.not_deleted(nil)`, whose scope is `-> { includes :user }`).
#[test]
fn preload_only_association_scope_still_seeds() {
    let show = shown();
    assert!(
        show.contains("Tag.alphabetical(ActiveRecord::Relation.new(Tag).where(recent_room_id: @room.id))"),
        "includes-only scope does not block the seed:\n{show}"
    );
}
