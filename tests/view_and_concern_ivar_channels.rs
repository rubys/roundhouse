//! Four channels an ivar travels that the seeding did not walk, each
//! reported to the user as `@x has no known type` about an ivar the
//! action two lines up assigns.
//!
//! 1. **A format variant.** Rails picks `create.turbo_stream.erb` over
//!    `create.html.erb` by the request's format; both are the same
//!    action's template, but the variant's view name carries the suffix
//!    and matched no seed. campfire's whole message-post response is
//!    that template.
//! 2. **An explicit render somewhere in the body.** One `render action:
//!    :not_found` in a `rescue` made `view_name_for_action` answer THAT
//!    template and only that one — so the action's own conventional
//!    template was fed by nothing, on every path that isn't the rescue.
//! 3. **A partial used as a LAYOUT.** `render layout: "x", locals: {…}
//!    do … end` renders `_x` in the caller's view context, reading the
//!    caller's ivars, exactly like `render partial:`. Only the
//!    `partial:` key was read.
//! 4. **A parent's refined binding, reaching a subclass.** Phase A
//!    types every body against an EMPTY ivar context to harvest
//!    bindings, so a binding that depends on another ivar comes out
//!    `Var`; Phase B fixes it into a local that the next round rebuilds
//!    from scratch. A subclass that calls `super` and then reads the
//!    ivar inherited the `Var`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::diagnose;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :rooms do |t|\n    t.string :name\n  end\n  \
    create_table :messages do |t|\n    t.integer :room_id\n    t.string :body\n  end\nend\n";

const ROOM: &str = "class Room < ApplicationRecord\n  has_many :messages\nend\n";
const MESSAGE: &str = "class Message < ApplicationRecord\n  belongs_to :room\nend\n";

const CONCERN: &str = r#"
module TrackedVisit
  extend ActiveSupport::Concern

  def remember_last_room_visited
    @room.name
  end
end
"#;

const PARENT: &str = r#"
class MessagesController < ApplicationController
  include TrackedVisit

  before_action :set_room

  def create
    @message = @room.messages.first
  rescue ActiveRecord::RecordNotFound
    render action: :not_found
  end

  private
    def set_room
      @room = Room.first
    end
end
"#;

/// The subclass defines `create` itself and calls `super`, so BOTH
/// bodies run and the parent's `@message` is in scope after it.
const CHILD: &str = r#"
class Messages::ByBotsController < MessagesController
  def create
    super
    head :created, location: @message.body
  end
end
"#;

fn diagnostics() -> Vec<String> {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :messages\nend\n",
        ),
        ("app/models/room.rb", ROOM),
        ("app/models/message.rb", MESSAGE),
        ("app/controllers/concerns/tracked_visit.rb", CONCERN),
        ("app/controllers/messages_controller.rb", PARENT),
        ("app/controllers/messages/by_bots_controller.rb", CHILD),
        // The conventional template, only as a FORMAT VARIANT.
        (
            "app/views/messages/create.turbo_stream.erb",
            "<%= @message.body %>\n",
        ),
        ("app/views/messages/not_found.html.erb", "<p>gone</p>\n"),
        // A partial used as a layout, reading the caller's ivar and a local.
        (
            "app/views/messages/index.html.erb",
            "<%= render layout: \"messages/shell\", locals: { room: @room } do %><p>x</p><% end %>\n",
        ),
        (
            "app/views/messages/_shell.html.erb",
            "<%= room.name %><%= @message.body %>\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    diagnose(&app).into_iter().map(|d| d.to_string()).collect()
}

fn errors_in(file: &str) -> Vec<String> {
    diagnostics()
        .into_iter()
        .filter(|d| d.starts_with("error") && d.contains(file))
        .collect()
}

#[test]
fn a_format_variant_template_is_fed_by_its_action() {
    let o = errors_in("create.turbo_stream.erb");
    assert!(
        o.is_empty(),
        "`messages/create.turbo_stream` is `create`'s template too: {o:?}"
    );
}

#[test]
fn a_partial_used_as_a_layout_gets_the_locals_and_the_ivars() {
    let o = errors_in("_shell.html.erb");
    assert!(
        o.is_empty(),
        "`render layout:` is the same channel as `render partial:`: {o:?}"
    );
}

#[test]
fn a_subclass_that_calls_super_sees_the_parents_binding() {
    let o = errors_in("by_bots_controller.rb");
    assert!(
        o.is_empty(),
        "`super` runs the parent's body, which assigns @message: {o:?}"
    );
}

#[test]
fn a_concern_reads_the_includers_ivar() {
    let o = errors_in("tracked_visit.rb");
    assert!(
        o.is_empty(),
        "the concern's methods run on the controller and read its ivars: {o:?}"
    );
}
