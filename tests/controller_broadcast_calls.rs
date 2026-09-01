//! Rails-API broadcast calls written from a CONTROLLER — the third home
//! these have, after a model's own body and a concern beside it.
//!
//! Emitted verbatim until `controller_to_library::broadcasts` existed,
//! i.e. as an undefined method: campfire's room controllers raise on
//! create/update/destroy in a real server, not only under test.
//!
//! Semantics measured against turbo-rails 2.0.23 + Rails 8.1, same as
//! `broadcast_calls.rs`: `target: [room, :list]` runs through
//! `dom_id(*target)` and the PREFIX COMES FIRST (`list_room_1`); a
//! record streamable contributes `<singular>_<id>`; and a `partial:`
//! renders exactly what `render partial:` renders.
//!
//! The stream name is the half that cannot be checked in isolation — it
//! must match what `turbo_stream_from` emits in the view or the
//! broadcast is published where nobody listens. Both sides go through
//! `lower::broadcasts::stream_name`; these tests pin the controller
//! side's spelling of it.

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

fn controller_src() -> String {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#,
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
        (
            "app/views/rooms/_row.html.erb",
            "<div id=\"<%= dom_id(room) %>\"><%= room.name %></div>\n",
        ),
        ("app/views/rooms/index.html.erb", "<h1>Rooms</h1>\n"),
        (
            "app/controllers/rooms_controller.rb",
            r#"class RoomsController < ApplicationController
  def index
    @room = Room.first
    broadcast_shared
    broadcast_per_user
    broadcast_gone
    broadcast_with_attributes
  end

  private
    def broadcast_shared
      broadcast_prepend_to :rooms, target: :shared_rooms,
        partial: "rooms/row", locals: { room: @room }
    end

    def broadcast_per_user
      user = User.first
      broadcast_replace_to user, :rooms, target: [ @room, :list ],
        partial: "rooms/row", locals: { room: @room }
    end

    def broadcast_gone
      broadcast_remove_to :rooms, target: [ @room, :list ]
    end

    def broadcast_with_attributes
      broadcast_append_to :rooms, target: :shared_rooms,
        partial: "rooms/row", locals: { room: @room },
        attributes: { maintain_scroll: true, tone: "quiet", missing: nil }
    end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // `emit_lowered_controllers_with_layout` is the entry the ruby
    // target uses for controllers — `emit_library` writes concerns and
    // POROs and knows nothing about them.
    let files = ruby::emit_lowered_controllers_with_layout(&app);
    let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
    files
        .iter()
        .find(|f| f.path.ends_with("rooms_controller.rb"))
        .unwrap_or_else(|| panic!("no rooms_controller.rb in {paths:?}"))
        .content
        .clone()
}

/// A literal streamable is its own text, and a `partial:` binds through
/// the same def-site contract `render partial:` uses.
#[test]
fn a_literal_stream_lowers_with_the_named_partial_as_its_payload() {
    let src = controller_src();
    assert!(
        src.contains(
            "Broadcasts.prepend(stream: \"rooms\", target: \"shared_rooms\", \
             html: ActionView::ViewHelpers.broadcast_render(-> { Views::Rooms.row(@room) }))"
        ),
        "{src}",
    );
}

/// A RECORD streamable contributes `<singular>_<id>` — the half that has
/// to agree with `turbo_stream_from` — and `target: [record, :prefix]`
/// spells the prefix FIRST.
#[test]
fn a_record_streamable_names_the_stream_and_the_target_puts_the_prefix_first() {
    let src = controller_src();
    assert!(
        src.contains(
            "Broadcasts.replace(stream: \"#{GlobalID.param(\"User\", user.id)}:rooms\", \
             target: \"list_#{@room.dom_prefix}_#{@room.dom_record_key}\""
        ),
        "{src}",
    );
}

/// `remove` carries no payload — there is nothing to render.
#[test]
fn remove_carries_no_html() {
    let src = controller_src();
    assert!(
        src.contains("Broadcasts.remove(stream: \"rooms\", target: \"list_#{@room.dom_prefix}_#{@room.dom_record_key}\")"),
        "{src}",
    );
    assert!(
        !src.contains("Broadcasts.remove(stream: \"rooms\", target: \"list_#{@room.dom_prefix}_#{@room.dom_record_key}\", html"),
        "remove must not carry html:\n{src}",
    );
}

/// `attributes:` rides on the turbo-stream ELEMENT, and turbo-rails
/// writes it AHEAD of `action`/`target` via `tag.turbo_stream(template,
/// **attributes, action:, target:)`. Rendered here rather than threaded
/// as a hash, because the value is a literal at every call site.
///
/// Measured against ActionView 8.1's `TagBuilder`: the key is written as
/// SPELLED (nothing dasherizes it — campfire's own JS reads
/// `hasAttribute("maintain_scroll")`), the value is `to_s` then escaped,
/// and a `nil` value omits the attribute.
#[test]
fn attributes_render_to_element_text_ahead_of_action_and_target() {
    let src = controller_src();
    assert!(
        src.contains(
            "Broadcasts.append(stream: \"rooms\", target: \"shared_rooms\", \
             html: ActionView::ViewHelpers.broadcast_render(-> { Views::Rooms.row(@room) }), \
             attributes: \" maintain_scroll=\\\"true\\\" tone=\\\"quiet\\\"\")"
        ),
        "{src}",
    );
}

/// A broadcast with no `attributes:` emits exactly the call it emitted
/// before the slot existed — the runtime parameter defaults to the same
/// empty string, so nothing else in the corpus moves.
#[test]
fn a_broadcast_without_attributes_gains_no_argument() {
    let src = controller_src();
    assert!(
        src.contains(
            "Broadcasts.prepend(stream: \"rooms\", target: \"shared_rooms\", \
             html: ActionView::ViewHelpers.broadcast_render(-> { Views::Rooms.row(@room) }))"
        ),
        "{src}",
    );
}

/// Nothing recognizable is left behind as an undefined method.
#[test]
fn no_broadcast_to_call_survives_into_the_emit() {
    let src = controller_src();
    assert!(!src.contains("broadcast_prepend_to"), "{src}");
    assert!(!src.contains("broadcast_replace_to"), "{src}");
    assert!(!src.contains("broadcast_remove_to"), "{src}");
}
