//! `record.update!(creator: user)` → `update!(creator_id: user.id)`
//! (`lower::apply_assoc_attr_key_lowering`).
//!
//! The synthesized `update` enumerates COLUMNS and virtual writers and
//! claims association names without emitting anything for them, so an
//! association key was SILENTLY DROPPED. campfire's
//! `rooms_controller_test` writes `rooms(:designers).update! creator:
//! users(:jz)` and then expects the new creator to be able to destroy
//! the room; the assignment vanished and the test read as a permissions
//! failure.
//!
//! Keyed on the NAME. The receivers at these sites analyze to
//! `None`/`Untyped`/an open var — a test body's `rooms(:designers)`
//! carries no stamped type — so a type gate fires on nothing. Two
//! models disagreeing on the foreign key, or a column anywhere sharing
//! the name, decline.

use std::collections::HashMap;
use std::path::PathBuf;

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
    t.integer "creator_id", null: false
  end
end
"#;

fn body_of(models: &[(&str, &str)], test: &str) -> String {
    let mut files: Vec<(&str, &str)> = vec![("db/schema.rb", SCHEMA)];
    files.extend_from_slice(models);
    let test_src = format!(
        "require \"test_helper\"\n\nclass RoomTest < ActiveSupport::TestCase\n  test \"x\" do\n    {test}\n  end\nend\n"
    );
    files.push(("test/models/room_test.rb", &test_src));
    let mut app = ingest_app_from_tree(tree(&files)).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app.test_modules
        .iter()
        .flat_map(|m| m.tests.iter().map(|t| format!("{:?}", t.body)))
        .collect::<Vec<_>>()
        .join("\n")
}

const ROOM: &str = r#"class Room < ApplicationRecord
  belongs_to :creator, class_name: "User"
end
"#;
const USER: &str = "class User < ApplicationRecord\nend\n";

#[test]
fn an_association_key_becomes_its_foreign_key_and_id() {
    let ir = body_of(
        &[("app/models/room.rb", ROOM), ("app/models/user.rb", USER)],
        "room.update!(creator: user)",
    );
    assert!(
        ir.contains(r#"Sym { value: Symbol("creator_id") }"#),
        "the key must become the foreign-key column:\n{ir}"
    );
    assert!(
        !ir.contains(r#"Sym { value: Symbol("creator") }"#),
        "and the association name must not survive:\n{ir}"
    );
    assert!(ir.contains(r#"Symbol("id")"#), "the value must become its id:\n{ir}");
}

/// A COLUMN sharing the name is not unambiguously an association key —
/// the synthesized `update`'s column loop already claims it.
#[test]
fn a_name_that_is_also_a_column_declines() {
    let ir = body_of(
        &[
            (
                "app/models/room.rb",
                "class Room < ApplicationRecord\n  belongs_to :name, class_name: \"User\"\nend\n",
            ),
            ("app/models/user.rb", USER),
        ],
        "room.update!(name: user)",
    );
    assert!(
        ir.contains(r#"Sym { value: Symbol("name") }"#),
        "`name` is a column on both tables — the key stays:\n{ir}"
    );
}

/// An explicit nil means NULL in Rails, and `nil.id` would raise.
#[test]
fn an_explicit_nil_declines() {
    let ir = body_of(
        &[("app/models/room.rb", ROOM), ("app/models/user.rb", USER)],
        "room.update!(creator: nil)",
    );
    assert!(
        ir.contains(r#"Sym { value: Symbol("creator") }"#),
        "an explicit nil must not be rewritten:\n{ir}"
    );
}

/// Only the mass-assignment entries. An ordinary call taking a hash
/// with a same-named key is not one.
#[test]
fn an_unrelated_method_is_untouched() {
    let ir = body_of(
        &[("app/models/room.rb", ROOM), ("app/models/user.rb", USER)],
        "room.notify(creator: user)",
    );
    assert!(
        ir.contains(r#"Sym { value: Symbol("creator") }"#),
        "`notify` is not a mass-assignment entry:\n{ir}"
    );
}
