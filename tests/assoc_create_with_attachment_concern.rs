//! Campfire's write path: `@room.messages.create_with_attachment!(message_params)`.
//!
//! The class method lives on `Message::Attachment::ClassMethods` and is
//! spliced onto `Message`. The emit must thread the association's
//! `where_scope` so the call does not land on the materialized Array
//! reader (`NoMethodError: create_with_attachment! for an instance of Array`).

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
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.string "body"
    t.string "client_message_id"
  end
end
"#,
        ),
        (
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  resources :rooms do
    resources :messages, only: :create
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
  include Message::Attachment
  belongs_to :room
end
"#,
        ),
        (
            "app/models/message/attachment.rb",
            r#"module Message::Attachment
  extend ActiveSupport::Concern

  module ClassMethods
    def create_with_attachment!(attributes)
      create!(attributes).tap(&:process_attachment)
    end
  end

  def process_attachment
  end
end
"#,
        ),
        (
            "app/models/webhook.rb",
            r#"class Webhook
  def self.deliver(room, user, attachment)
    room.messages.create_with_attachment!(attachment: attachment, creator: user)
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
  end

  private
    def message_params
      params.require(:message).permit(:body, :attachment, :client_message_id)
    end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(suffix: &str) -> String {
    let app = app();
    let files = if suffix.contains("controller") {
        ruby::emit_lowered_controllers(&app)
    } else {
        ruby::emit_lowered_models(&app)
    };
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ending in {suffix}; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

#[test]
fn concern_create_with_attachment_takes_assoc_scope() {
    let message = emitted("app/models/message.rb");
    assert!(
        message.contains(
            "def self.create_with_attachment!(attributes, __rel = ActiveRecord::Relation.new(self))"
        ),
        "spliced concern method must take the association relation:\n{message}"
    );
    assert!(
        message.contains("scope_attributes.merge(attributes)")
            || message.contains("__rel.scope_attributes.merge(attributes)"),
        "create must merge association scope under attributes:\n{message}"
    );
}

#[test]
fn controller_create_threads_where_scope_not_array_reader() {
    let ctrl = emitted("app/controllers/messages_controller.rb");
    assert!(
        !ctrl.contains("@room.messages.create_with_attachment!"),
        "must not call create_with_attachment! on the Array reader:\n{ctrl}"
    );
    assert!(
        ctrl.contains("Message.create_with_attachment!")
            && ctrl.contains("where_scope(room_id: @room.id)"),
        "must re-root through Message + where_scope:\n{ctrl}"
    );
}
