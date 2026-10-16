//! `message.boosts.loaded?` → `message.boosts_loaded?` — the Rails
//! AssociationProxy spelling flattened onto Roundhouse's synthesized
//! flag (see `lower::assoc_loaded`).

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

#[test]
fn assoc_loaded_rewrites_two_hop_to_flat_predicate() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
    t.datetime "created_at"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
  scope :ordered, -> { order(created_at: :asc) }
end
"#,
        ),
        (
            "app/views/messages/boosts/_boosts.html.erb",
            r#"<%= render partial: "messages/boosts/boost", collection: message.boosts.loaded? ? message.boosts.sort_by(&:created_at) : message.boosts.ordered %>
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let views = ruby::emit_lowered_views(&app);
    let src = views
        .iter()
        .find(|f| f.path.to_string_lossy().contains("boosts"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("boosts_loaded?"),
        "expected message.boosts.loaded? to flatten to boosts_loaded?:\n{src}"
    );
    assert!(
        !src.contains(".loaded?"),
        "AssociationProxy .loaded? must not survive emit:\n{src}"
    );
}

#[test]
fn assoc_loaded_rewrites_implicit_self_two_hop() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts

  def boosts_ready?
    boosts.loaded?
  end
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("message.rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("boosts_loaded?"),
        "expected boosts.loaded? to flatten to boosts_loaded?:\n{src}"
    );
    assert!(
        !src.contains(".loaded?"),
        "implicit-self AssociationProxy .loaded? must not survive:\n{src}"
    );
}

#[test]
fn association_target_collapses_to_reader() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "action_text_rich_texts", force: :cascade do |t|
    t.string "name"
    t.text "body"
    t.string "record_type"
    t.integer "record_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_rich_text :body

  def indexed_text_changed?
    association(:rich_text_body).target&.saved_change_to_body?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("message.rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        !src.contains("association(:rich_text_body)"),
        "association(:rich_text_body).target must not survive emit:\n{src}"
    );
    assert!(
        src.contains("rich_text_body"),
        "expected collapse onto rich_text_body reader:\n{src}"
    );
}

#[test]
fn association_target_collapses_inside_concern_module() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "action_text_rich_texts", force: :cascade do |t|
    t.string "name"
    t.text "body"
    t.string "record_type"
    t.integer "record_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  include Message::Searchable
  has_rich_text :body
end
"#,
        ),
        (
            "app/models/message/searchable.rb",
            r#"module Message::Searchable
  extend ActiveSupport::Concern

  def indexed_text_changed?
    association(:rich_text_body).target&.saved_change_to_body?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_library(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("searchable"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        !src.contains("association(:rich_text_body)"),
        "concern-module association(:rich_text_body).target must collapse:\n{src}"
    );
    assert!(
        src.contains("rich_text_body"),
        "expected collapse onto rich_text_body reader inside concern:\n{src}"
    );
}

#[test]
fn association_target_skips_reader_on_unrelated_model() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  def wrong_target
    # `:boosts` is Message's has_many — must not collapse onto Room.
    association(:boosts).target
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room.rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("association(:boosts)"),
        "association(:boosts).target on Room must stay (boosts is Message-only):\n{src}"
    );
}

#[test]
fn assoc_loaded_skips_untyped_explicit_receiver() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  def check(owner)
    owner.boosts.loaded?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room.rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains(".loaded?"),
        "untyped owner.boosts.loaded? must stay (no unique-name guess):\n{src}"
    );
    assert!(
        !src.contains("boosts_loaded?"),
        "must not flatten onto boosts_loaded? for an untyped receiver:\n{src}"
    );
}

#[test]
fn assoc_loaded_skips_mixed_owner_union() {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  def check(flag)
    owner = flag ? Message.first : Room.first
    owner.boosts.loaded?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_lowered_models(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room.rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains(".loaded?"),
        "Message|Room union must keep .loaded? (only Message has boosts):\n{src}"
    );
    assert!(
        !src.contains("boosts_loaded?"),
        "must not flatten mixed-owner union onto boosts_loaded?:\n{src}"
    );
}

#[test]
fn assoc_loaded_mixed_owner_union_residuals_under_check() {
    // Twin of `assoc_loaded_skips_mixed_owner_union` for analyze/check:
    // Message|Room must not type as Bool (only Message has boosts).
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
  def check(flag)
    owner = flag ? Message.first : Room.first
    owner.boosts.loaded?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = roundhouse::analyze::diagnose(&app);
    assert!(
        diags.iter().any(|d| d.to_string().contains("loaded?")),
        "mixed-owner union must residual under check (not quiet Bool): {diags:?}"
    );
}

#[test]
fn assoc_loaded_two_hop_types_as_bool_under_check() {
    // `check` analyzes without post-analyze lowerings; the two-hop must
    // still type as Bool so tip Campfire views are not residual.
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.string "body"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
    t.datetime "created_at"
  end
end
"#,
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  has_many :boosts
end
"#,
        ),
        (
            "app/models/boost.rb",
            r#"class Boost < ApplicationRecord
  belongs_to :message
end
"#,
        ),
        (
            "app/views/messages/boosts/_boosts.html.erb",
            r#"<%= message.boosts.loaded? ? "y" : "n" %>
"#,
        ),
    ]))
    .expect("ingest");
    let mut analyzer = roundhouse::analyze::Analyzer::new(&app);
    analyzer.analyze(&mut app);
    let diags = roundhouse::analyze::diagnose(&app);
    let loaded = diags
        .iter()
        .filter(|d| d.to_string().contains("loaded?"))
        .collect::<Vec<_>>();
    assert!(
        loaded.is_empty(),
        "check-shaped analyze must type message.boosts.loaded?: {loaded:?}"
    );
}

#[test]
fn association_loaded_rewrites_shared_name_in_test_body() {
    // Campfire: Message and Membership both `belongs_to :room`. Unique-name
    // fails, but every matching model exposes `room_loaded?` — test bodies
    // with an untyped `message` local must still flatten.
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.integer "room_id"
  end
  create_table "memberships", force: :cascade do |t|
    t.integer "room_id"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  belongs_to :room
end
"#,
        ),
        (
            "app/models/membership.rb",
            r#"class Membership < ApplicationRecord
  belongs_to :room
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
end
"#,
        ),
        (
            "test/models/message_room_loaded_test.rb",
            r#"require "test_helper"

class MessageRoomLoadedTest < ActiveSupport::TestCase
  test "room starts unloaded" do
    message = Message.new
    assert_not message.association(:room).loaded?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // Full spinel emit reaches `app.test_modules` (models-only does not).
    let files = ruby::emit_spinel(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("message_room_loaded"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("room_loaded?"),
        "shared :room must flatten association(:room).loaded? in tests:\n{src}"
    );
    assert!(
        !src.contains("association(:room)"),
        "association(:room).loaded? must not survive test emit:\n{src}"
    );
}

#[test]
fn association_loaded_rewrites_after_reload_typed_as_base() {
    // Campfire mentionee: `message = ….create!(…).reload` then
    // `message.association(:room).loaded?`. `reload` types as Base, which
    // does not declare `:room` — must still flatten in test bodies.
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.integer "room_id"
  end
  create_table "memberships", force: :cascade do |t|
    t.integer "room_id"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  belongs_to :room
end
"#,
        ),
        (
            "app/models/membership.rb",
            r#"class Membership < ApplicationRecord
  belongs_to :room
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
            "test/models/message_room_loaded_test.rb",
            r#"require "test_helper"

class MessageRoomLoadedTest < ActiveSupport::TestCase
  test "room starts unloaded after reload" do
    message = Message.create!(room_id: 1).reload
    assert_not message.association(:room).loaded?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_spinel(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("message_room_loaded"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("room_loaded?"),
        "Base-typed reload receiver must still flatten association(:room).loaded?:\n{src}"
    );
    assert!(
        !src.contains("association(:room)"),
        "association(:room).loaded? must not survive after reload:\n{src}"
    );
}

#[test]
fn association_loaded_keeps_poly_receiver_when_sibling_declares_name() {
    // Message has polymorphic `:room` (no flat `room_loaded?`); Membership
    // has a supported `belongs_to :room`. The any-model gate must not rewrite
    // a Message-typed receiver onto a predicate it does not own.
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.integer "room_id"
    t.string "room_type"
  end
  create_table "memberships", force: :cascade do |t|
    t.integer "room_id"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        ),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  belongs_to :room, polymorphic: true
end
"#,
        ),
        (
            "app/models/membership.rb",
            r#"class Membership < ApplicationRecord
  belongs_to :room
end
"#,
        ),
        (
            "app/models/room.rb",
            r#"class Room < ApplicationRecord
end
"#,
        ),
        (
            "test/models/message_poly_room_loaded_test.rb",
            r#"require "test_helper"

class MessagePolyRoomLoadedTest < ActiveSupport::TestCase
  test "poly room stays on association.loaded?" do
    message = Message.new
    assert_not message.association(:room).loaded?
  end
end
"#,
        ),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_spinel(&app);
    let src = files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("message_poly_room_loaded"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    assert!(
        src.contains("association(:room)"),
        "polymorphic Message must keep association(:room).loaded?:\n{src}"
    );
    assert!(
        !src.contains("room_loaded?"),
        "must not flatten onto Membership's room_loaded? for a Message recv:\n{src}"
    );
}

#[test]
fn association_loaded_over_a_symbol_list_unrolls_onto_the_flat_predicates() {
    // Campfire's `message_fragment_cache_key`: the names are a literal
    // list and `association` is handed the block local, which nothing can
    // resolve. Unrolled, each term flattens like a literal-name call, and
    // `has_one_attached :attachment` answers `attachment_attachment`.
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "messages", force: :cascade do |t|
    t.integer "room_id"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id"
  end
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
end
"#,
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        (
            "app/models/message.rb",
            r#"class Message < ApplicationRecord
  belongs_to :room
  has_many :boosts
  has_one_attached :attachment

  def presentation_loaded?
    %i[room boosts attachment_attachment].all? { |name| association(name).loaded? }
  end
end
"#,
        ),
        ("app/models/boost.rb", "class Boost < ApplicationRecord\n  belongs_to :message\nend\n"),
        ("app/models/room.rb", "class Room < ApplicationRecord\n  has_many :messages\nend\n"),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let files = ruby::emit_spinel(&app);
    let model = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("models/message.rb"))
        .map(|f| f.content.as_str())
        .unwrap_or("");
    let body: String = model
        .lines()
        .skip_while(|l| !l.contains("def presentation_loaded?"))
        .take(3)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        body.contains("room_loaded? && ")
            && body.contains("boosts_loaded? && ")
            && body.contains("attachment_attachment_loaded?"),
        "the symbol list unrolls onto the flat predicates:\n{body}"
    );
    assert!(!body.contains("association("), "no dynamic association call survives:\n{body}");
    assert!(model.contains("def attachment_attachment_loaded?"), "{model}");
}
