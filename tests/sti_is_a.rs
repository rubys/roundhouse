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
    emitted_with(room, &[])
}

fn emitted_with(room: &str, extra: &[(String, String)]) -> String {
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
    .chain(extra.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())))
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

/// The names a row can carry for `is_a?(Chain::L1)` are the subclass and
/// every STI descendant, however deep the chain. An STI subclass is one
/// whose parent chain reaches the model within nine hops, so `L9` is the
/// deepest and `L10` is not an STI subclass at all.
#[test]
fn an_sti_test_names_every_descendant_in_a_deep_chain() {
    let extra: Vec<(String, String)> = (1..=11)
        .map(|i| {
            let parent = if i == 1 { "::Room".to_string() } else { format!("Chain::L{}", i - 1) };
            (
                format!("app/models/chain/l{i}.rb"),
                format!("module Chain\n  class L{i} < {parent}\n  end\nend\n"),
            )
        })
        .collect();
    let room = "class Room < ApplicationRecord\n  def deep?\n    is_a?(Chain::L1)\n  end\nend\n";
    let src = emitted_with(room, &extra);
    let line = src.lines().find(|l| l.contains("Chain::L1\"") && l.contains("include?")).expect("the deep? test").trim().to_string();
    assert!(line.contains("\"Chain::L9\""), "{line}");
    assert!(!line.contains("\"Chain::L10\""), "ten hops from the model is past the STI bound:\n{line}");
}
