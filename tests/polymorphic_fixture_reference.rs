//! A polymorphic fixture reference writes BOTH of its columns
//! (`lower::fixtures::resolve_field`).
//!
//! Rails spells the pair as one key:
//!
//! ```text
//! first:
//!   record: first (Message)
//! ```
//!
//! which is `record_id` AND `record_type`. The resolver answered one
//! field per key and matched only the non-polymorphic `belongs_to`
//! shape (a bare label), so the whole entry fell through and the row
//! landed keyed to nothing — campfire's `action_text/rich_texts.yml`,
//! whose thirteen records are every message's body.
//!
//! Writing only the id half would be worse than writing neither: a row
//! keyed to the right id under no type belongs to every model at once.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted() -> String {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"messages\", force: :cascade do |t|\n    t.integer \"room_id\", null: false\n  end\n  create_table \"action_text_rich_texts\", force: :cascade do |t|\n    t.text \"body\"\n    t.string \"name\", null: false\n    t.integer \"record_id\", null: false\n    t.string \"record_type\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/message.rb"),
            b"class Message < ApplicationRecord\n  has_rich_text :body\nend\n".to_vec(),
        ),
        (
            PathBuf::from("test/fixtures/messages.yml"),
            b"first:\n  room_id: 1\n\nsecond:\n  room_id: 1\n".to_vec(),
        ),
        (
            PathBuf::from("test/fixtures/action_text/rich_texts.yml"),
            b"first:\n  record: first (Message)\n  name: body\n  body: First post!\n\nsecond:\n  record: second (Message)\n  name: body\n  body: Seconded.\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("action_text_rich_texts.rb"))
        .map(|f| f.content)
        .expect("rich-text fixture emitted")
}

#[test]
fn a_polymorphic_reference_writes_the_id_and_the_type() {
    let src = emitted();
    assert!(
        src.contains("instance.record_id = 1") && src.contains("instance.record_type = \"Message\""),
        "both halves of `record: first (Message)`:\n{src}"
    );
    assert!(
        src.contains("instance.record_id = 2"),
        "…resolved per record, not once:\n{src}"
    );
}

/// The plain columns beside it still land — the same entry point had to
/// keep working for keys that name one column.
#[test]
fn the_scalar_columns_beside_it_still_land() {
    let src = emitted();
    assert!(
        src.contains("instance.name = \"body\"") && src.contains("instance.body = \"First post!\""),
        "scalar columns unaffected:\n{src}"
    );
}
