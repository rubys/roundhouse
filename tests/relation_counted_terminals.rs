//! `first(n)` / `last(n)` on a relation are different methods from the
//! bare forms (`scope_chain::counted_terminal`).
//!
//! Rails' counted forms answer an Array of up to n records; the bare
//! forms answer one record or nil. One method cannot carry both return
//! types on a strict target, so the runtime splits them into `first_n` /
//! `last_n` and the call site is renamed here.
//!
//! The rename is gated on the receiver having been PROVEN a relation,
//! because `Array#first(n)` and `String#split.last(n)` mean exactly what
//! Rails means and must survive untouched — lobsters'
//! `parsed.to_html.split.first(words * 2)` is the shape a receiver-blind
//! rename would corrupt.

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
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
    t.integer "room_id", null: false
    t.string "created_at", null: false
  end
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  has_many :messages
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  PAGE_SIZE = 20

  scope :ordered, -> { order(:created_at) }
  scope :last_page, -> { ordered.last(PAGE_SIZE) }
  scope :first_page, -> { ordered.first(PAGE_SIZE) }
  scope :newest, -> { ordered.last }
end
"#,
        ),
        (
            "app/controllers/rooms_controller.rb",
            r#"class RoomsController < ApplicationController
  def show
    @room = Room.find(params[:id])
    @head = @room.messages.ordered.first(3)
    @words = summary.split.first(4).join(" ")
    @tail = summary.split.last(2).join(" ")
  end

  private
    def summary
      "a b c d e f"
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

/// The shape that sent me here: campfire's
/// `scope :last_page, -> { ordered.last(PAGE_SIZE) }`, where the receiver
/// is the threaded `__rel`.
#[test]
fn counted_terminal_on_a_threaded_relation_is_renamed() {
    let message = emitted(&ruby::emit_lowered_models(&app()), "app/models/message.rb");
    assert!(
        message.contains("Message.ordered(__rel).last_n(PAGE_SIZE)"),
        "last(n) on a relation becomes last_n:\n{message}"
    );
    assert!(
        message.contains("Message.ordered(__rel).first_n(PAGE_SIZE)"),
        "first(n) on a relation becomes first_n:\n{message}"
    );
}

/// The bare forms answer one record and keep their names — only the
/// counted forms are split.
#[test]
fn bare_terminal_keeps_its_name() {
    let message = emitted(&ruby::emit_lowered_models(&app()), "app/models/message.rb");
    assert!(
        message.contains("Message.ordered(__rel).last\n")
            || message.contains("Message.ordered(__rel).last "),
        "zero-arg last is untouched:\n{message}"
    );
    assert!(!message.contains(".last_n\n"), "no arg-less last_n:\n{message}");
}

/// A counted terminal on a has_many read rides the FK seed.
///
/// The chain names a scope (`ordered`), which is what opens
/// `apply_scope_lowering`'s gate for this body — a body whose ONLY
/// relation surface were the bare `@room.messages.first(3)` is not
/// rewritten at all today; see the note on `mentions_assoc_constructor`.
#[test]
fn counted_terminal_on_a_seeded_association_is_renamed() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains("Message.ordered(ActiveRecord::Relation.new(Message).where(room_id: @room.id)).first_n(3)"),
        "@room.messages.ordered.first(3) seeds and renames:\n{show}"
    );
}

/// The gate. `String#split` answers an Array, whose `first(n)`/`last(n)`
/// already mean what Rails means — renaming them would call a method
/// Array does not have.
#[test]
fn counted_terminal_on_a_non_relation_receiver_is_left_alone() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains("split.first(4)") && show.contains("split.last(2)"),
        "Array receivers keep Array#first/#last:\n{show}"
    );
}
