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
  scope :search, ->(q) { where("body like ?", q) }
  scope :shared, -> { where(room_id: 1) }
end
"#,
        ),
        (
            // A SECOND model declaring `shared` — the name now names
            // nothing, which is what pins the uniqueness guard below.
            "app/models/note.rb",
            r#"class Note < ApplicationRecord
  scope :shared, -> { where(room_id: 1) }
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
    @found = anything.search("hi").last(100)
    @shared = anything.shared.last(100)
  end

  private
    def summary
      "a b c d e f"
    end

    # Deliberately untypeable — the point of the two assertions that
    # read through it is that the RECEIVER's type never resolves.
    def anything
      Current.whatever
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
        show.contains("Message.ordered(ActiveRecord::Relation.new(Message).where(room_id: @room.id).preloaded(@room.messages_target, @room.messages_loaded?)).first_n(3)"),
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


/// A receiver whose own type never resolves, but whose OUTERMOST call
/// NAMES A SCOPE, is a relation — a scope returns one by construction.
///
/// campfire's search page is
/// `Current.user.reachable_messages.search(query).last(100)`, where
/// `Current.user` is untyped at harvest (an ivar on a lowered
/// CurrentAttributes class) and takes the whole chain down with it. At
/// run time every link answers a real Relation; only this rename was
/// missing, so the call landed on the runtime's ZERO-ARG `last`.
#[test]
fn a_receiver_naming_a_scope_is_a_relation_even_when_untyped() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains(".search(\"hi\").last_n(100)"),
        "a scope-named receiver renames the counted terminal:\n{show}"
    );
}

/// The guard: a scope name TWO models declare names nothing, so it
/// proves nothing about the receiver. Same standard
/// `owner_model_from_name` holds association names to.
#[test]
fn a_scope_name_two_models_share_proves_nothing() {
    let show = emitted(
        &ruby::emit_lowered_controllers(&app()),
        "app/controllers/rooms_controller.rb",
    );
    assert!(
        show.contains(".shared.last(100)"),
        "an ambiguous scope name must not rename:\n{show}"
    );
    assert!(
        !show.contains(".shared.last_n(100)"),
        "an ambiguous scope name must not rename:\n{show}"
    );
}
