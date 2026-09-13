//! A record Array at a Relation-typed parameter is restated as the
//! Relation over those records (`lower::records_to_relation_arg`).
//!
//! `find_or_create_for(users)` is typed `Relation[User]` from its one
//! app caller; campfire's tests pass `[ users(:david), users(:kevin) ]`.
//! Ruby plucks either, a typed emit picks the app's shape, and the
//! test's literal becomes `User.where(id: [david.id, kevin.id])`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted(test_body: &str, model_call: &str) -> String {
    let test_src = format!(
        "require \"test_helper\"\n\nclass RoomTest < ActiveSupport::TestCase\n  test \"one\" do\n    {test_body}\n  end\nend\n"
    );
    let controller_src = format!(
        "class RoomsController < ApplicationController\n  def create\n    @room = Room.{model_call}\n    redirect_to rooms_url\n  end\nend\n"
    );
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\n  create_table \"rooms\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\n  create_table \"users\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\n  resources :rooms, only: %i[ create ]\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/user.rb"),
            b"class User < ApplicationRecord\nend\n".to_vec(),
        ),
        (
            PathBuf::from("app/models/room.rb"),
            b"class Room < ApplicationRecord\n  def self.find_or_create_for(users)\n    ids = users.pluck(:id)\n    Room.create!(name: ids.join(\",\"))\n  end\n\n  def self.named(name)\n    Room.create!(name: name)\n  end\nend\n".to_vec(),
        ),
        (PathBuf::from("app/controllers/rooms_controller.rb"), controller_src.into_bytes()),
        (PathBuf::from("test/fixtures/users.yml"), b"david:\n  name: David\nkevin:\n  name: Kevin\n".to_vec()),
        (PathBuf::from("test/models/room_test.rb"), test_src.into_bytes()),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_spinel(&app)
        .iter()
        .find(|f| f.path.ends_with("room_test.rb"))
        .expect("no room_test.rb emitted")
        .content
        .clone()
}

#[test]
fn a_record_array_at_a_relation_param_becomes_the_relation_over_its_ids() {
    let src = emitted(
        "room = Room.find_or_create_for([ users(:david), users(:kevin) ])\n    assert room",
        "find_or_create_for(User.where(id: params[:user_ids]))",
    );
    assert!(
        src.contains(
            "Room.find_or_create_for(User.where(id: [UsersFixtures.david.id, UsersFixtures.kevin.id]))"
        ),
        "the literal should be restated as the Relation over its ids:\n{src}"
    );
}

#[test]
fn a_literal_at_a_param_that_is_not_a_relation_is_left_alone() {
    let src = emitted(
        "room = Room.named([ users(:david) ].size.to_s)\n    assert room",
        "named(params[:name])",
    );
    assert!(
        src.contains("Room.named([ UsersFixtures.david ].size.to_s)"),
        "a String-typed parameter has no Relation to restate into:\n{src}"
    );
    assert!(!src.contains(".where(id:"), "{src}");
}
