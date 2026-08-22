//! A class method called BOTH with a params helper and with a plain
//! attribute hash (`params_merge`'s `Binding::Attrs`).
//!
//! campfire's `Message.create_with_attachment!(attributes)` is reached
//! from `MessagesController` with `message_params` and from `Webhook`
//! with `attachment: …, creator: …`. One parameter, two argument shapes,
//! and nothing infers a parameter's type from its call sites — so the
//! method's `create!(attributes)` handed a params object to
//! `initialize(attrs)` and indexed a class with no `[]`.
//!
//! Monomorphizing into two methods was the alternative. It is rejected
//! here because the app has ONE concept — the parameter is named
//! `attributes` and both callers mean an attribute hash — and the params
//! object is the side that knows how to become one. So the helper site
//! converts (`to_attrs`, presence-guarded and Symbol-keyed, which is
//! exactly what `initialize` consumes) and the parameter is DECLARED a
//! hash, which is in turn what lets the association-scope pass merge a
//! foreign key into it.
//!
//! `<Model>.create(<helper>)` is deliberately NOT converted: that call
//! is already monomorphized by name (`create_from_params` for the params
//! site, the runtime's `create(attrs)` for the hash site), and rewriting
//! it would trade a typed factory for a bag.

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

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.string "body", null: false
    t.string "client_message_id", null: false
    t.integer "room_id", null: false
  end
  create_table "notes", force: :cascade do |t|
    t.string "text", null: false
    t.integer "room_id", null: false
  end
end
"#;

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  include Retitling

  has_many :messages
  has_many :notes
end
"#,
        ),
        // A CONCERN with an instance method whose body needs a Hash.
        // Its one call site passes a params helper, so the call-site
        // census alone concludes "params object"; the body is what says
        // otherwise.
        (
            "app/models/room/retitling.rb",
            r#"module Room::Retitling
  extend ActiveSupport::Concern

  def retitle!(attributes)
    attributes.delete(:draft)
    update!(attributes)
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  def self.create_with_attachment!(attributes)
    create!(attributes)
  end
end
"#,
        ),
        (
            "app/models/note.rb",
            r#"class Note < ApplicationRecord
  def self.file!(attributes)
    create!(attributes)
  end
end
"#,
        ),
        (
            "app/models/webhook.rb",
            r#"class Webhook
  def self.deliver(room)
    room.messages.create_with_attachment!(client_message_id: "bot")
  end
end
"#,
        ),
        (
            "app/controllers/messages_controller.rb",
            r#"class MessagesController < ApplicationController
  def create
    @room = Room.find(params[:room_id])
    @message = @room.messages.create_with_attachment!(message_params)
    @note = @room.notes.file!(note_params)
  end

  def rename
    @room = Room.find(params[:room_id])
    @room.retitle! room_params
  end

  private
    def message_params
      params.require(:message).permit(:body, :client_message_id)
    end

    def note_params
      params.require(:note).permit(:text)
    end

    def room_params
      params.require(:room).permit(:name)
    end
end
"#,
        ),
    ]))
    .expect("ingest")
}

/// The pass under test is a POST-ANALYZE lowering, so the fixture has to
/// go through the session, not bare ingest.
fn lowered() -> roundhouse::App {
    let mut app = app();
    roundhouse::session::analyze_and_lower(&mut app);
    app
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

fn controller() -> String {
    let app = lowered();
    emitted(
        &ruby::emit_lowered_controllers(&app),
        "app/controllers/messages_controller.rb",
    )
}

fn model(name: &str) -> String {
    let app = lowered();
    emitted(&ruby::emit_lowered_models(&app), name)
}

/// Params classes are synthesized by the controller lowering, so they
/// ride out with the lowered controllers.
fn params_class(name: &str) -> String {
    let app = lowered();
    emitted(&ruby::emit_lowered_controllers(&app), name)
}

/// The site the two shapes meet: the helper converts, and the
/// association scope rides in beside it.
#[test]
fn the_params_site_converts_to_an_attribute_hash() {
    let create = controller();
    assert!(
        create.contains(
            "Message.create_with_attachment!(self.message_params.to_attrs, \
             ActiveRecord::Relation.new(Message).where_scope(room_id: @room.id))"
        ),
        "the helper converts and the association scope is threaded:\n{create}"
    );
}

/// `to_attrs` is Symbol-keyed and presence-guarded — the shape
/// `initialize(attrs)` consumes. A field the request did not send is
/// OMITTED, not written as `""`, or it would overwrite the column
/// default on create.
#[test]
fn to_attrs_omits_what_was_not_provided() {
    let params = params_class("message_params.rb");
    assert!(
        params.contains("attrs[:body] = @body if @body_provided"),
        "presence-guarded Symbol-keyed entries:\n{params}"
    );
    assert!(
        params.contains("def to_attrs"),
        "the method is synthesized on demand:\n{params}"
    );
}

/// The callee's parameter is DECLARED a hash, which is what lets the
/// scope pass merge the association's foreign key into the create
/// instead of declining.
#[test]
fn the_callee_takes_the_scope_because_its_parameter_is_a_hash() {
    let message = model("app/models/message.rb");
    assert!(
        message.contains(
            "def self.create_with_attachment!(attributes, __rel = ActiveRecord::Relation.new(self))"
        ),
        "the method takes the association's relation:\n{message}"
    );
    assert!(
        message.contains("create!(__rel.scope_attributes.merge(attributes))"),
        "and merges the scope under the caller's own attributes:\n{message}"
    );
}

/// A method reached with ONLY a params helper keeps the params object —
/// nothing converts, because nothing proved the parameter is a hash.
#[test]
fn a_single_shape_callee_is_left_alone() {
    let create = controller();
    assert!(
        !create.contains("note_params.to_attrs"),
        "a callee nobody passes a hash to is untouched:\n{create}"
    );
    let note = model("app/models/note.rb");
    assert!(
        !note.contains("scope_attributes"),
        "and it takes no scope — its `create!` argument is not provably a hash:\n{note}"
    );
}

/// THE BODY DECIDES, when the call sites cannot.
///
/// `Room#retitle!(attributes)` has one call site and it passes a params
/// helper, so the census concludes "this parameter is a params object"
/// and rewrites the body to consume one. But the body opens with
/// `attributes.delete(:draft)`, which no params object answers. A
/// hash-only use in the callee's own body forces the attribute-hash
/// binding, so the call site converts instead.
///
/// Reaching it at all takes two other things: the receiver is an IVAR,
/// resolved through its stamped type, and the method lives in a CONCERN,
/// so it has to be filed under the model that `include`s it.
#[test]
fn a_body_that_needs_a_hash_binds_one_however_its_call_sites_look() {
    let rename = controller();
    assert!(
        rename.contains("room_params.to_attrs"),
        "an ivar receiver's concern method converts at the call site:\n{rename}"
    );
}

/// `to_attrs` is synthesized only where a call site asks for it: the
/// demand is read back off the rewritten controller body, the same way
/// `wants_create` is read off `<Model>.create(<helper>)`.
#[test]
fn to_attrs_is_demand_gated() {
    let params = params_class("note_params.rb");
    assert!(
        !params.contains("def to_attrs"),
        "a list nobody converts carries no to_attrs:\n{params}"
    );
}
