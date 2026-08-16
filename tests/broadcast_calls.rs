//! Rails-API broadcast calls written as ORDINARY METHODS —
//! campfire's `Message::Broadcasts` concern, which no callback walk
//! reaches.
//!
//! Semantics measured against turbo-rails 2.0.23 + Rails 8.1:
//! `target: [room, :messages]` runs through `dom_id(*target)`, whose
//! PREFIX COMES FIRST (`messages_room_1`); `broadcast_remove_to`
//! defaults its target to `dom_id(self)` while the insert actions
//! default to `model_name.plural`; and with no `partial:` the payload
//! is the record's own partial.
//!
//! The stream name is the half that cannot be checked in isolation: it
//! has to match what `turbo_stream_from` emits in the VIEW, or the
//! message is published where nobody is listening. Both sides call
//! `lower::broadcasts::stream_name`, and the last test here pins them
//! together.

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

fn lowered() -> Vec<roundhouse::emit::EmittedFile> {
    // `emit_library` is what writes a concern MODULE — the home
    // campfire's broadcasts actually live in.
    ruby::emit_library(&lowered_app())
}

fn lowered_app() -> roundhouse::App {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.string "body", null: false
  end
end
"#,
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\n  has_many :messages\nend\n"),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  include Message::Broadcasts

  belongs_to :room
end
"#,
        ),
        (
            "app/models/message/broadcasts.rb",
            r#"module Message::Broadcasts
  def broadcast_create
    broadcast_append_to room, :messages, target: [ room, :messages ]
  end

  def broadcast_remove
    broadcast_remove_to room, :messages
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn find(files: &[roundhouse::emit::EmittedFile], name: &str) -> String {
    files
        .iter()
        .find(|f| f.path.ends_with(name))
        .unwrap_or_else(|| panic!("no {name} in {:?}", files.iter().map(|f| &f.path).collect::<Vec<_>>()))
        .content
        .clone()
}

#[test]
fn append_lowers_to_a_broadcasts_call_with_the_records_own_partial() {
    let src = find(&lowered(), "broadcasts.rb");
    assert!(
        src.contains(
            "Broadcasts.append(stream: \"room_#{bc_owner.id}:messages\", \
             target: \"messages_room_#{bc_owner.id}\", html: Views::Messages.message(self))"
        ),
        "{src}",
    );
}

/// The association is read ONCE. The stream name and the DOM target both
/// want the owner's id, and on most targets reading an association is a
/// query.
#[test]
fn the_record_streamable_binds_to_one_local_and_is_nil_guarded() {
    let src = find(&lowered(), "broadcasts.rb");
    assert!(src.contains("bc_owner = room"), "{src}");
    assert!(src.contains("return if bc_owner.nil?"), "{src}");
    assert_eq!(src.matches("bc_owner = room").count(), 2, "once per method:\n{src}");
}

/// `broadcast_remove_to` defaults its target to the record itself, not
/// to the plural the insert actions use — measured, and the asymmetry is
/// easy to get wrong.
#[test]
fn remove_defaults_its_target_to_the_record_and_carries_no_html() {
    let src = find(&lowered(), "broadcasts.rb");
    assert!(
        src.contains(
            "Broadcasts.remove(stream: \"room_#{bc_owner.id}:messages\", target: \"message_#{@id}\")"
        ),
        "{src}",
    );
}

/// The `/cable` endpoint, the `Broadcasts.set_transport` registration in
/// config.ru and the `websocket-driver` gem all ride ONE predicate, and
/// it used to ask whether a model DECLARED broadcasts. This app declares
/// none — the calls are ordinary methods on a concern — so the tree
/// shipped `Broadcasts.append` with nothing to deliver it: the POST wrote
/// its row and no second tab ever heard about it.
#[test]
fn a_broadcast_written_as_a_plain_method_still_needs_the_cable_endpoint() {
    let app = lowered_app();
    assert!(
        app.models
            .iter()
            .all(|m| roundhouse::lower::lower_broadcasts(m).is_empty()),
        "no model here DECLARES a broadcast — that is exactly the case that regressed",
    );
    assert!(roundhouse::lower::app_broadcasts_live(&app));
}

/// The other direction, and the reason the predicate can't just be
/// `true`: an app that never broadcasts still ships cable-free (no
/// endpoint, no gem — issue #67).
#[test]
fn an_app_that_never_broadcasts_stays_cable_free() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    assert!(!roundhouse::lower::app_broadcasts_live(&app));
}

/// The two ends of the wire. A publish-side name that does not match the
/// subscribe side is invisible — the page renders, the broadcast runs,
/// and nothing arrives — so this pins them to the same spelling.
#[test]
fn the_stream_name_matches_what_a_view_subscribes_to() {
    use roundhouse::expr::{Expr, ExprNode, Literal};
    use roundhouse::lower::broadcasts::{stream_name, Streamable};

    let id = Expr::new(
        roundhouse::span::Span::synthetic(),
        ExprNode::Lit { value: Literal::Int { value: 1 } },
    );
    let name = stream_name(&[
        Streamable::Record { singular: "room".into(), id },
        Streamable::Literal("messages".into()),
    ]);
    let ExprNode::StringInterp { parts } = &*name.node else {
        panic!("expected an interpolation, got {name:?}");
    };
    assert_eq!(parts.len(), 3, "{parts:?}");

    // An all-literal name stays a plain String — the blog's
    // `turbo_stream_from "articles"` has always emitted the literal.
    let plain = stream_name(&[Streamable::Literal("articles".into())]);
    assert!(
        matches!(&*plain.node, ExprNode::Lit { value: Literal::Str { value } } if value == "articles"),
        "{plain:?}",
    );
}
