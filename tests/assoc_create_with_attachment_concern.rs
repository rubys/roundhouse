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

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

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
        // Campfire's suite passes a local `attributes` hash — that used to
        // set saw_other and collapse the Attrs binding so assoc-scope
        // declined and the controller kept the Array-reader call.
        (
            "test/models/message_attachment_test.rb",
            r#"require "test_helper"

class MessageAttachmentTest < ActiveSupport::TestCase
  test "create" do
    attributes = { body: "hi", client_message_id: "x" }
    rooms(:hq).messages.create_with_attachment!(attributes)
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
                files
                    .iter()
                    .map(|f| f.path.display().to_string())
                    .collect::<Vec<_>>()
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
        message.contains("create!(__rel.scope_attributes.merge(attributes))"),
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

#[test]
fn emitted_concern_create_persists_the_association_foreign_key() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.string "body"
    t.string "client_message_id"
  end
end
"#,
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/room.rb",
            "class Room < ApplicationRecord\n  has_many :messages\nend\n",
        )
        .write(
            "app/models/message.rb",
            "class Message < ApplicationRecord\n  include Message::Attachment\n  belongs_to :room\nend\n",
        )
        .write(
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
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms do\n    resources :messages, only: :create\n  end\nend\n",
        )
        .write(
            "app/controllers/messages_controller.rb",
            r#"class MessagesController < ApplicationController
  def create
    @room = Room.find(params[:room_id])
    @message = @room.messages.create_with_attachment!(message_params)
  end

  private
    def message_params
      params.require(:message).permit(:body, :client_message_id)
    end
end
"#,
        )
        .run_ruby(r#"
room = Room.create!(name: "hq")
require_relative "app/controllers/messages_controller"
ActionController::Base.allow_forgery_protection = false
controller = MessagesController.new
ActionController::Current.controller = controller
controller.request = ActionDispatch::TestRequest.create("HTTP_HOST" => "app.example", "REQUEST_METHOD" => "POST")
ActionController::Current.request = controller.request
controller.request_method = "POST"
controller.params = {
  "room_id" => room.id.to_s,
  "message" => { "body" => "hello", "client_message_id" => "runtime" }
}
controller.process_action(:create)
message = Message.where(room_id: room.id).first
raise "controller create did not persist a message" unless message
raise "association foreign key was not threaded" unless message.room_id == room.id
raise "message was not persisted" unless Message.where(room_id: room.id).count == 1
puts "concern association create passed"
"#)
        .assert_passes();
}
