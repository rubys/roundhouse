//! A bare call to a FRAMEWORK-generated preload scope inside another
//! scope's body must receive the threaded relation.
//!
//! Rails evaluates a scope lambda with `self` as the current relation,
//! so a bare `with_attached_attachment` inside `scope :x, -> { … }`
//! chains onto whatever the caller had built. campfire writes exactly
//! that:
//!
//! ```text
//! scope :with_attachment_details, -> {
//!   with_rich_text_body_and_embeds
//!   with_attached_attachment
//!     .includes(attachment_blob: :variant_records)
//! }
//! ```
//!
//! `with_attached_attachment` is synthesized beside `has_one_attached`,
//! not declared by the app, so it was absent from
//! `build_scope_registry`. The bare call fell through every arm of the
//! scope-body rewriter, took its own `__rel` default — a FRESH relation
//! — and every `where` the caller had accumulated vanished.
//!
//! THE SYMPTOM WAS WRONG ROWS, NOT SLOW ONES. campfire's `rooms#show`
//! builds `where(room_id: @room.id)` and pipes it through this scope;
//! `/rooms/1` served the last 40 messages in the whole table, which
//! were room 5's. Every tag count matched Rails because every room is
//! seeded to the same shape — a tag-tally oracle cannot see this class
//! of bug, and this test is what stands in for one.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.datetime "created_at", null: false
  end
  create_table "action_text_rich_texts", force: :cascade do |t|
    t.text "body"
    t.string "name", null: false
    t.bigint "record_id", null: false
    t.string "record_type", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#;

const MESSAGE: &str = r#"class Message < ApplicationRecord
  belongs_to :room
  has_one_attached :attachment
  has_rich_text :body

  scope :ordered, -> { order(:created_at) }
  scope :with_attachment_details, -> {
    with_rich_text_body_and_embeds
    with_attached_attachment
      .includes(attachment_blob: :variant_records)
  }
end
"#;

fn emitted() -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\n  has_many :messages\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/message.rb"), MESSAGE.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.ends_with("message.rb"))
        .expect("no message.rb emitted")
        .content
        .clone()
}

#[test]
fn the_bare_preload_scope_call_is_threaded() {
    let src = emitted();
    let at = src
        .find("def self.with_attachment_details")
        .unwrap_or_else(|| panic!("{src}"));
    let body = &src[at..src[at..].find("\n  end").map(|i| at + i).unwrap_or(src.len())];
    // Threaded, not defaulted: a bare `with_attached_attachment` here
    // would build its own relation and lose the caller's `where`.
    assert!(body.contains("with_attached_attachment(__rel)"), "{body}");
    assert!(body.contains("with_rich_text_body_and_embeds(__rel)"), "{body}");
}

/// The delegate the same name gets ON A RELATION preloads the same
/// association the class-side body does, with no `__scope_` hop: there
/// is no arity to detect and no model to pick. Registering these in the
/// scope registry (which the threading above needs) must not turn that
/// into the general path.
#[test]
fn the_relation_delegate_preloads() {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\n  has_many :messages\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/message.rb"), MESSAGE.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let src = ruby::emit_spinel(&app)
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with("relation_scopes.rb"))
        .expect("relation_scopes.rb emitted")
        .content;
    let at = src.find("def with_attached_attachment").unwrap_or_else(|| panic!("{src}"));
    assert!(
        src[at..].starts_with("def with_attached_attachment\n      preload(:attachment_attachment)\n"),
        "{}",
        &src[at..(at + 80).min(src.len())]
    );
    let at = src.find("def with_rich_text_body_and_embeds").unwrap_or_else(|| panic!("{src}"));
    assert!(
        src[at..].starts_with("def with_rich_text_body_and_embeds\n      preload(:rich_text_body)\n"),
        "{}",
        &src[at..(at + 80).min(src.len())]
    );
}

/// What the preload spec lands on: the model carries a batch loader for
/// the attachment under Rails' association name, and the dispatch
/// answers it. This is the machinery that takes campfire's room page
/// from 80 attachment lookups to one.
#[test]
fn the_attachment_preload_has_a_batch_loader() {
    let src = emitted();
    assert!(
        src.contains("def self._preload_batch_attachment_attachment(records)"),
        "{src}"
    );
    assert!(src.contains("when :attachment_attachment"), "{src}");
    assert!(
        src.contains("FROM active_storage_attachments a JOIN active_storage_blobs b"),
        "one join for the whole record set:\n{src}"
    );
    assert!(src.contains("r._preload_attachment_attachment(att)"), "{src}");
}

/// The rich-text twin: one `IN` over `action_text_rich_texts`, handed to
/// the owner's load-once setter, with `ActionText::RichText` as the
/// nested-preload target.
#[test]
fn the_rich_text_preload_has_a_batch_loader() {
    let src = emitted();
    assert!(src.contains("def self._preload_batch_rich_text_body(records)"), "{src}");
    assert!(src.contains("when :rich_text_body"), "{src}");
    assert!(
        src.contains(r#"ActiveRecord::Relation.new(ActionText::RichText).where(record_type: "Message", name: "body", record_id: ids)"#),
        "{src}"
    );
    assert!(src.contains("r._preload_rich_text_body(by_id[r.id])"), "{src}");
    assert!(src.contains("def _preload_rich_text_body(rec)"), "the owner's setter is emitted:\n{src}");
}
