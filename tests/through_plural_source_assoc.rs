//! `has_many :through` whose `source:` names a HAS_MANY on the join
//! model — campfire's `User has_many :reachable_messages, through:
//! :rooms, source: :messages`.
//!
//! Two independent bugs met on that one line, and together they made a
//! reader that ran and answered wrong-or-not-at-all:
//!
//! - INGEST derived the target class by camelizing the source name.
//!   That is right for the singular sources lobsters writes (`source:
//!   :story` → Story) and wrong for a plural one: `:messages` became a
//!   `Messages` phantom. campfire HAS a `Messages::` controller module
//!   by that name, so the emitted `Messages.where(...)` resolved to a
//!   module and died at `where` rather than at the missing constant —
//!   two controller test files behind it.
//! - The join-chain resolver knew two shapes, `belongs_to` on the join
//!   model and a nested `:through`. A plain `has_many` is a third: the
//!   fk lives on the TARGET table, so the join points the other way.
//!   Without it the chain went unresolved and the shared direct-fk
//!   reader stayed, querying a `user_id` column messages does not have.

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

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "memberships", force: :cascade do |t|
    t.integer "user_id", null: false
    t.integer "room_id", null: false
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.integer "creator_id", null: false
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  has_many :memberships, dependent: :delete_all
  has_many :rooms, through: :memberships
  has_many :reachable_messages, through: :rooms, source: :messages
end
"#,
        ),
        (
            "app/models/membership.rb",
            r#"class Membership < ApplicationRecord
  belongs_to :user
  belongs_to :room
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  has_many :memberships, dependent: :delete_all
  has_many :messages, dependent: :destroy
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  belongs_to :room
  belongs_to :creator, class_name: "User", foreign_key: :creator_id
end
"#,
        ),
    ]))
    .expect("ingest plural-source through app")
}

fn model_src(name: &str) -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(name))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {name}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>(),
            )
        })
}

/// The source class comes from the SINGULAR of the source name — no
/// `Messages` phantom, and no module of that name to collide with.
#[test]
fn a_plural_source_resolves_to_the_singular_class() {
    let src = model_src("user.rb");
    assert!(
        src.contains("ActiveRecord::Relation.new(Message)"),
        "reachable_messages must seed a Message relation:\n{src}"
    );
    assert!(
        !src.contains("Messages."),
        "no `Messages` phantom class:\n{src}"
    );
}

/// User → memberships → rooms → messages. The room hop reverses,
/// because `messages.room_id` is the key, not `rooms.message_id`.
#[test]
fn a_has_many_source_joins_from_the_target_side() {
    let src = model_src("user.rb");
    assert!(
        src.contains(
            "INNER JOIN rooms ON rooms.id = messages.room_id \
             INNER JOIN memberships ON memberships.room_id = rooms.id"
        ),
        "the has_many source hop must join from the target table:\n{src}"
    );
    assert!(src.contains("memberships.user_id = ?"), "{src}");
    // The silently-wrong direct-fk fallback (messages has no user_id).
    assert!(
        !src.contains("where({ user_id: @id })"),
        "shared direct-fk reader must be replaced:\n{src}"
    );
}

/// The plain two-hop through beside it is untouched.
#[test]
fn the_belongs_to_source_hop_is_unchanged() {
    let src = model_src("user.rb");
    assert!(
        src.contains("INNER JOIN memberships ON memberships.room_id = rooms.id"),
        "{src}"
    );
}
