//! PostgreSQL: sti is a postgres, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).

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

const ROOM: &str = "class Room < ApplicationRecord\n  def open?\n    is_a?(Rooms::Open)\n  end\nend\n";

fn emitted(with_database_yml: bool) -> String {
    let mut files = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", ROOM),
        ("app/models/rooms/open.rb", "module Rooms\n  class Open < ::Room\n  end\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  resources :rooms\nend\n"),
    ];
    if with_database_yml {
        files.push(("roundhouse.yml", ""));
        files.push(("config/database.yml", "test:\n  adapter: postgresql\n  database: roundhouse_unused\n"));
    }
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_models(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("room.rb"))
        .map(|f| f.content.clone())
        .expect("room.rb")
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_postgresql_tree_keeps_is_a_against_an_sti_subclass() {
    let src = emitted(true);
    assert!(src.contains("is_a?(Rooms::Open)"), "the subclass hydrates, so is_a? answers:\n{src}");
    assert!(!src.contains("type =="), "no inheritance-column read:\n{src}");
}

#[test]
fn a_tree_without_the_postgresql_runtime_still_reads_the_column() {
    let src = emitted(false);
    assert!(src.contains("type == \"Rooms::Open\""), "{src}");
}
