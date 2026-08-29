//! `pluck` on rows the pipeline already materialized.
//!
//! campfire's `Message::Broadcasts#broadcast_unread_room` writes
//! `room.memberships.pluck(:user_id)`. `Room#memberships` is a lowered
//! reader that answers a hydrated `Array[Membership]`, and Array has no
//! `pluck` — a 500 on every `POST /rooms/:id/messages` in a served app,
//! which the suite never sees because the job that reaches the line is
//! enqueued under the test adapter rather than run.
//!
//! THE RECEIVER'S TYPE IS THE DISCRIMINATOR. A plain `has_many` reader
//! hydrates and answers an Array; a `has_many :through` answers an
//! `ActiveRecord::Relation`, which HAS `pluck` and whose `pluck` is a
//! single-column SELECT. Both are spelled `owner.name`, so nothing
//! syntactic tells them apart.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "memberships", force: :cascade do |t|
    t.integer "room_id", null: false
    t.integer "user_id", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

fn emit(files: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut all: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ];
    all.extend_from_slice(files);
    let tree: HashMap<PathBuf, Vec<u8>> = all
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    // Models go through `emit_lowered_models`; a concern or PORO would
    // land in `emit_library`. Both, so a test can put its call site in
    // whichever is natural for the shape it is describing.
    ruby::emit_lowered_models(&app)
        .into_iter()
        .chain(ruby::emit_library(&app))
        .map(|f| (f.path.to_string_lossy().to_string(), f.content))
        .collect()
}

fn file_ending(files: &[(String, String)], suffix: &str) -> String {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.clone())
        .unwrap_or_else(|| {
            panic!("no {suffix}; got {:?}", files.iter().map(|(p, _)| p).collect::<Vec<_>>())
        })
}

const USER: &str = "class User < ApplicationRecord\nend\n";
const MEMBERSHIP: &str =
    "class Membership < ApplicationRecord\n  belongs_to :room\n  belongs_to :user\nend\n";

#[test]
fn a_hydrated_association_readers_pluck_becomes_a_projection() {
    let files = emit(&[
        ("app/models/user.rb", USER),
        ("app/models/membership.rb", MEMBERSHIP),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  has_many :memberships\n\n  \
             def member_ids\n    memberships.pluck(:user_id)\n  end\nend\n",
        ),
    ]);
    let room = file_ending(&files, "room.rb");
    assert!(
        room.contains("map { |__pluck| __pluck.user_id }"),
        "a plain has_many reader answers an Array, which has no `pluck`:\n{room}"
    );
    assert!(
        !room.contains("memberships.pluck"),
        "the pluck must not survive on the Array:\n{room}"
    );
}

/// A `has_many :through` reader answers an `ActiveRecord::Relation`,
/// whose `pluck` is a single-column SELECT. Rewriting THAT one trades a
/// projection for a whole-row hydrate — the opposite of the fix.
#[test]
fn a_relation_valued_reader_keeps_its_pluck() {
    let files = emit(&[
        ("app/models/membership.rb", MEMBERSHIP),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  has_many :memberships\n  \
             has_many :rooms, through: :memberships\n\n  \
             def room_names\n    rooms.pluck(:name)\n  end\nend\n",
        ),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  has_many :memberships\nend\n",
        ),
    ]);
    let user = file_ending(&files, "user.rb");
    assert!(
        user.contains("pluck(:name)"),
        "a Relation already answers `pluck`, and better:\n{user}"
    );
    assert!(
        !user.contains("__pluck"),
        "a Relation's single-column SELECT must not become a hydrate:\n{user}"
    );
}

/// A model CONST root is the scope-chain seeder's, and a Const-rooted
/// chain is arel's — both do better than a map.
#[test]
fn a_const_rooted_chain_is_left_to_arel() {
    let files = emit(&[
        ("app/models/user.rb", USER),
        ("app/models/membership.rb", MEMBERSHIP),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  has_many :memberships\n\n  \
             def self.all_names\n    Room.where(name: \"x\").pluck(:name)\n  end\nend\n",
        ),
    ]);
    let room = file_ending(&files, "room.rb");
    assert!(
        !room.contains("__pluck"),
        "an inlined single-column SELECT must not become a whole-table hydrate:\n{room}"
    );
}

/// Rails answers an Array of VALUES for one column and an Array of
/// ARRAYS for several. Getting that wrong would be silent.
#[test]
fn a_multi_column_pluck_projects_a_row() {
    let files = emit(&[
        ("app/models/user.rb", USER),
        ("app/models/membership.rb", MEMBERSHIP),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  has_many :memberships\n\n  \
             def pairs\n    memberships.pluck(:user_id, :room_id)\n  end\nend\n",
        ),
    ]);
    let room = file_ending(&files, "room.rb");
    assert!(
        room.contains("[__pluck.user_id, __pluck.room_id]"),
        "several columns are a row per element:\n{room}"
    );
}

/// `pluck("users.id")` and `pluck(:"users.id")` name a column through
/// SQL, not through a reader — nothing answers those on a row.
#[test]
fn a_qualified_column_name_is_left_alone() {
    let files = emit(&[
        ("app/models/user.rb", USER),
        ("app/models/membership.rb", MEMBERSHIP),
        (
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  has_many :memberships\n\n  \
             def qualified\n    memberships.pluck(:\"memberships.user_id\")\n  end\nend\n",
        ),
    ]);
    let room = file_ending(&files, "room.rb");
    assert!(
        !room.contains("__pluck"),
        "a table-qualified name is not a method on a row:\n{room}"
    );
}
