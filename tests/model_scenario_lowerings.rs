//! Four gaps once-campfire-rust's model scenario found when
//! `scripts/campfire-db-differential` ran it through the transpiled
//! campfire — each compiled clean and failed at runtime, which is the
//! shape invariant 6 warns about. Pinned here on a small app, since the
//! differential itself needs the Rails oracle:
//!
//!   * an association constructor on an owner EXPRESSION
//!     (`Room.find(id).messages.create!(…)`) stayed on the plain
//!     reader's Array — `create!` for an instance of Array;
//!   * a local assigned inside `begin … rescue` read as unresolved after
//!     it, so the same rewrite on `m.boosts.create!` silently declined;
//!   * `pluck(:id)` on a parameter that holds an Array had no Array
//!     `pluck` to answer it;
//!   * the mocha slot guard `lower::mocha` prepends to a stubbed app
//!     method named `MochaStub`, which only the test helper loaded — the
//!     production boots now load it too;
//!   * destroying a record left its `has_rich_text` row behind (the
//!     macro's `dependent: :destroy` was not expanded);
//!   * an attribute-hash `update!(status: …)` wrote `created_at` back
//!     EMPTY — its temporal normalize was guarded by a `.to_s.nil?` that
//!     is never true.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name"
  end
  create_table "messages", force: :cascade do |t|
    t.integer "room_id", null: false
    t.string "body"
  end
  create_table "boosts", force: :cascade do |t|
    t.integer "message_id", null: false
    t.string "content"
  end
  create_table "users", force: :cascade do |t|
    t.string "name"
    t.integer "status", default: 0, null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
  create_table "notes", force: :cascade do |t|
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
  create_table "action_text_rich_texts", force: :cascade do |t|
    t.string "name", null: false
    t.text "body"
    t.string "record_type", null: false
    t.bigint "record_id", null: false
    t.datetime "created_at", null: false
    t.datetime "updated_at", null: false
  end
end
"#;

fn app() -> roundhouse::App {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/models/room.rb", "class Room < ApplicationRecord\n  has_many :messages\nend\n"),
        (
            "app/models/message.rb",
            "class Message < ApplicationRecord\n  belongs_to :room\n  has_many :boosts\nend\n",
        ),
        ("app/models/boost.rb", "class Boost < ApplicationRecord\n  belongs_to :message\nend\n"),
        ("app/models/note.rb", "class Note < ApplicationRecord\n  has_rich_text :content\nend\n"),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  def deactivate
    update!(status: 1)
  end

  def self.ids_of(users)
    users.pluck(:id)
  end
end
"#,
        ),
        (
            "app/controllers/scenarios_controller.rb",
            r#"class ScenariosController < ApplicationController
  def show
    first = Room.find(1).messages.create!(body: "one")
    begin
      second = Room.find(2).messages.create!(body: "two")
    rescue StandardError => e
      Rails.logger.info(e.message)
    end
    second.boosts.create!(content: "hi")
    ids = User.ids_of([User.find(1), User.find(2)])
    render plain: first.id.to_s + ids.length.to_s
  end
end
"#,
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .into_iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    ingest_app_from_tree(tree).expect("ingest")
}

fn emitted(files: Vec<roundhouse::emit::EmittedFile>, suffix: &str) -> String {
    files
        .into_iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content)
        .unwrap_or_else(|| panic!("no emitted file ending in {suffix}"))
}

fn controller() -> String {
    emitted(ruby::emit_lowered_controllers(&app()), "app/controllers/scenarios_controller.rb")
}

#[test]
fn a_constructor_on_an_owner_expression_names_the_owner_once() {
    let src = controller();
    assert!(
        // The Ruby family hands the owner OBJECT to the automatic
        // inverse (`Message belongs_to :room`), as Rails sets it; the
        // writer stores the same foreign key.
        src.contains("Message.create!(body: \"one\", room: Room.find(1))"),
        "the owner expression goes to the inverse, evaluated once:\n{src}"
    );
    assert!(!src.contains(".messages.create!"), "no constructor may stay on the reader:\n{src}");
}

#[test]
fn a_local_assigned_in_a_begin_keeps_its_type_after_it() {
    let src = controller();
    assert!(
        src.contains("Boost.create!(content: \"hi\", message: second)"),
        "`second` is still a Message after the begin, so its constructor rewrites:\n{src}"
    );
}

#[test]
fn pluck_on_a_parameter_holding_an_array_projects_with_map() {
    // `lower::assoc_pluck` is a session pass, not an emit-time one.
    let mut lowered = app();
    roundhouse::session::analyze_and_lower(&mut lowered);
    let models = emitted(ruby::emit_lowered_models(&lowered), "app/models/user.rb");
    let ids_of = models.split("def self.ids_of").nth(1).unwrap_or_else(|| panic!("{models}"));
    assert!(
        ids_of.contains("users.map { |__pluck| __pluck.id }"),
        "an Array has no pluck; map answers both an Array and a Relation:\n{ids_of}"
    );
}

#[test]
fn both_production_boots_load_the_mocha_slot() {
    let fixture = roundhouse::fixtures::real_blog().to_path_buf();
    let mut blog = roundhouse::ingest::ingest_app(&fixture).expect("ingest real-blog");
    roundhouse::session::analyze_and_lower(&mut blog);
    for target in [BuildTarget::Spinel, BuildTarget::Ruby] {
        let files = target_files(&blog, &fixture, target).expect("target files");
        let file = |p: &str| files.iter().find(|(path, _)| path == p).map(|(_, c)| c.clone());
        assert!(file("runtime/mocha_stub.rb").is_some(), "{target:?} ships mocha_stub.rb");
        let boot = file("boot.rb").unwrap_or_else(|| panic!("{target:?} has a boot.rb"));
        assert!(
            boot.contains("require_relative \"runtime/mocha_stub\""),
            "{target:?}'s production boot loads the slot its app code names"
        );
    }
}

fn lowered_model(suffix: &str) -> String {
    let mut lowered = app();
    roundhouse::session::analyze_and_lower(&mut lowered);
    emitted(ruby::emit_lowered_models(&lowered), suffix)
}

#[test]
fn destroying_a_record_destroys_its_rich_text() {
    let note = lowered_model("app/models/note.rb");
    let hook = note.split("def before_destroy").nth(1).unwrap_or_else(|| panic!("no before_destroy:\n{note}"));
    assert!(hook.contains("_destroy_rich_text_content"), "the cascade runs before destroy:\n{note}");
    let cascade = note.split("def _destroy_rich_text_content").nth(1).unwrap_or_else(|| panic!("{note}"));
    assert!(
        cascade.contains("record_type = 'Note'") && cascade.contains("name = 'content'"),
        "it destroys this record's rows for this attribute only:\n{cascade}"
    );
    assert!(cascade.contains(".each { |rich_text| rich_text.destroy }"), "{cascade}");
}

#[test]
fn a_hash_update_writes_created_at_only_when_named() {
    let user = lowered_model("app/models/user.rb");
    let update = user.split("def update!(attrs)").nth(1).unwrap_or_else(|| panic!("{user}"));
    let body = update.split("\n  end\n").next().unwrap();
    assert!(!body.contains(".to_s.nil?"), "no guard that can never fail:\n{body}");
    let normalize = body
        .lines()
        .find(|l| l.contains("created_at_raw = ActiveSupport.format_db_time"))
        .unwrap_or_else(|| panic!("no created_at normalize:\n{body}"));
    assert!(normalize.contains("attrs.key?(:created_at)"), "guarded by the key:\n{normalize}");
}
