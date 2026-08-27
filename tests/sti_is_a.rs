//! `room.is_a?(Rooms::Open)` is a read of the inheritance COLUMN.
//!
//! An STI object's class IS what its `type` column said when the row
//! loaded, so the column comparison is the same answer on every target
//! — the ruby-family lanes hydrate the subclass, the strict ones do
//! not, and `type` is what both of them have. campfire's `Room#open?` /
//! `#closed?` / `#direct?` are three of these and they steer real
//! behaviour.
//!
//! Spelling `self.` on the receiverless form was the fix NOT taken: it
//! compiles on spinel and answers FALSE for a subclass receiver when
//! the method sits on the base class (probed, filed upstream). A
//! predicate that says an open room is not open is worse than a build
//! that stops.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
    t.string "type"
  end
end
"#;

fn emitted(room: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", room),
        (
            "app/models/rooms/open.rb",
            "module Rooms\n  class Open < ::Room\n  end\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room.rb"))
        .map(|f| f.content.clone())
        .expect("room.rb")
}

const ROOM: &str = r#"class Room < ApplicationRecord
  def open?
    is_a?(Rooms::Open)
  end

  def named_like?(other)
    other.is_a?(String) && name == other
  end
end
"#;

#[test]
fn a_receiverless_sti_test_becomes_the_type_column() {
    let src = emitted(ROOM);
    assert!(
        src.contains("type == \"Rooms::Open\""),
        "the STI test reads the inheritance column:\n{src}"
    );
    assert!(
        !src.contains("is_a?(Rooms::Open)"),
        "no `is_a?` against an STI subclass may survive:\n{src}"
    );
}

/// An ordinary type test is somebody else's business.
#[test]
fn a_non_sti_type_test_is_left_alone() {
    let src = emitted(ROOM);
    assert!(
        src.contains("is_a?(String)"),
        "`other.is_a?(String)` is not an STI test and stays exactly as written:\n{src}"
    );
}
