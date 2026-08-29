//! `pluck(:col)` / `ids` fold into the SELECT, they don't dangle on a
//! hydrated Array.
//!
//! campfire's `User#grant_membership_to_open_rooms` writes
//! `Membership.insert_all(Rooms::Open.pluck(:id).collect { … })`.
//! `sti_scope` rewrites the subclass root to
//! `Room.where(type: "Rooms::Open")`, the arel pass materialized THAT
//! into a hydrate loop, and `.pluck(:id)` was left hanging on the
//! resulting Array — `undefined method 'each' for unknown` two links
//! down the chain, on the first signed-in request campfire serves.
//!
//! The projection is where `pluck` belongs (it is what Rails emits
//! too): one column in the SELECT, one positional column read per row,
//! and an `Array[<column ty>]` out.
//!
//! `count` rides along because it is the same missing arm one link
//! over: the base recognizer answers `Model.count`, but a `count`
//! LAYERED on a chain hydrated every matching row and counted the
//! Array — the right number over a whole-row fetch. lobsters'
//! `InvitationRequest.verified_count` is that site.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::session::analyze_and_lower;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
    t.string "type", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "email", null: false
    t.string "bio"
  end
end
"#,
        ),
        (
            // The campfire site is a model callback, and that is not
            // incidental: the arel pass runs BEFORE `type_method_body`
            // in `model_to_library` and AFTER it in
            // `controller_to_library`, and the typer demotes a trailing
            // kwargs hash to positional when the callee's signature
            // declares a positional Hash — which `Base.self.where` does.
            // So a `where` chain lifts in a model and does not in a
            // controller. Fixture written where the corpus site is.
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  def self.open_ids
    Rooms::Open.pluck(:id)
  end

  def self.open_count
    Rooms::Open.count
  end

  def self.first_five_count
    Room.where(name: "x").limit(5).count
  end

  def self.all_ids
    Room.ids
  end

  def self.names
    Room.pluck("name")
  end

  def self.pairs
    Room.pluck(:id, :name)
  end

  def self.missing
    Room.pluck(:nonexistent)
  end
end
"#,
        ),
        ("app/models/rooms/open.rb", "class Rooms::Open < Room\nend\n"),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  def self.bios
    User.pluck(:bio)
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    // The campfire site arrives at the arel pass already re-rooted by
    // `sti_scope`, so the test runs the whole post-analyze seam rather
    // than the one pass — the interaction between the two IS the bug.
    analyze_and_lower(&mut app);
    app
}

fn model(suffix: &str) -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| panic!("no emitted file ending in {suffix}"))
}

fn room() -> String {
    model("app/models/room.rb")
}

#[test]
fn sti_pluck_folds_into_the_select() {
    let room = room();
    assert!(
        room.contains("SELECT id FROM rooms WHERE type = 'Rooms::Open'"),
        "pluck(:id) is the projection, and the STI scope survives it:\n{room}"
    );
    assert!(
        room.contains("results << Db.column_int(stmt, 0)"),
        "the row read is a positional column read, not a hydrate:\n{room}"
    );
    // The dangling-refiner shape this closes.
    assert!(
        !room.contains("results.pluck"),
        "no pluck survives on a materialized Array:\n{room}"
    );
    assert!(
        !room.contains("SELECT id, name, type FROM rooms WHERE type"),
        "no whole-row fetch under a pluck:\n{room}"
    );
}

#[test]
fn ids_is_pluck_of_the_primary_key() {
    let room = room();
    assert!(
        room.contains("SELECT id FROM rooms\""),
        "`ids` names its own column:\n{room}"
    );
}

#[test]
fn a_string_column_name_names_the_same_column() {
    let room = room();
    assert!(
        room.contains("SELECT name FROM rooms"),
        r#"pluck("name") is pluck(:name):"#
    );
}

#[test]
fn a_nullable_column_reads_through_the_opt_variant() {
    let user = model("app/models/user.rb");
    assert!(
        user.contains("SELECT bio FROM users"),
        "nullable column still folds:\n{user}"
    );
    assert!(
        user.contains("Db.column_text_opt(stmt, 0)"),
        "a NULL must arrive as nil, not as the empty string:\n{user}"
    );
}

#[test]
fn multi_column_pluck_stays_on_the_relation_path() {
    let room = room();
    // Two columns of different types is a shape the strict targets have
    // no shared vocabulary for — decline, don't guess.
    assert!(
        room.contains("pluck(:id, :name)"),
        "multi-column pluck is left whole for the runtime:\n{room}"
    );
    assert!(
        !room.contains("SELECT id, name FROM rooms"),
        "no multi-column projection is built:\n{room}"
    );
}

#[test]
fn a_column_the_table_lacks_is_declined() {
    let room = room();
    assert!(
        !room.contains("SELECT nonexistent"),
        "a pluck of an unknown column must not compose SQL:\n{room}"
    );
}

/// `count` layered on a chain. `Model.count` was already the base arm;
/// `Model.where(…).count` hydrated every matching row and counted the
/// Array — the right answer over a whole-row fetch Rails never does.
#[test]
fn a_counted_chain_counts_in_sql() {
    let room = room();
    assert!(
        room.contains("SELECT COUNT(*) FROM rooms WHERE type = 'Rooms::Open'"),
        "count folds into the projection:\n{room}"
    );
    assert!(
        !room.contains("SELECT id, name, type FROM rooms WHERE type"),
        "no whole-row fetch under a count:\n{room}"
    );
}

/// `limit(5).count` is `min(5, total)` in Rails, and `emit_count`
/// renders no LIMIT — so the chain is declined rather than answered
/// with the unlimited total.
#[test]
fn a_limited_count_is_declined() {
    let room = room();
    assert!(
        !room.contains("SELECT COUNT(*) FROM rooms WHERE name"),
        "a limited count must not fold to an unlimited COUNT(*):\n{room}"
    );
    assert!(
        room.contains("WHERE name = 'x' LIMIT 5") && room.contains("results.count"),
        "the limited chain still lifts; only the counting stays in Ruby:\n{room}"
    );
}
