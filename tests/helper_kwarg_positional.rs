//! A helper's NAMED keyword, passed as a keyword, bound to the wrong
//! thing.
//!
//! Ingest lowers an optional keyword parameter to a
//! positional-with-default; the call sites kept the keyword, so Ruby
//! bound the trailing `{for_user: nil}` HASH to the positional
//! `for_user`. campfire's sidebar then reached
//! `room.users.without(for_user)` and emitted
//! `NOT (id.for_user IS NULL)` — SQL naming a column that does not
//! exist.
//!
//! The rule is NAMES: a trailing-kwarg key that matches a positional
//! parameter name means that parameter. Keys that match nothing —
//! `id:`/`class:` against a `**attributes` helper — mean a hash, and
//! are left alone by construction.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :rooms do |t|\n    t.string :name\n  end\nend\n";

fn emit(helper: &str, view: &str) -> String {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/room.rb"),
        b"class Room < ApplicationRecord\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :rooms\nend\n".to_vec(),
    );
    tree.insert(
        PathBuf::from("app/controllers/rooms_controller.rb"),
        b"class RoomsController < ApplicationController\n  def index\n    @rooms = Room.all\n  end\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/helpers/rooms_helper.rb"), helper.as_bytes().to_vec());
    tree.insert(PathBuf::from("app/views/rooms/index.html.erb"), view.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    // The VIEW is where the call site lives; the helper module emits
    // separately and would show only the definition.
    ruby::emit_lowered_views(&app)
        .into_iter()
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

const NAMED_KWARG: &str =
    "module RoomsHelper\n  def label_for(room, upcase: false)\n    upcase ? room.name.upcase : room.name\n  end\nend\n";

/// A key matching a parameter name binds POSITIONALLY.
#[test]
fn a_named_keyword_moves_into_its_positional_slot() {
    let src = emit(NAMED_KWARG, "<%= label_for(@rooms.first, upcase: true) %>\n");
    assert!(
        src.contains("label_for(@rooms.first, true)") || src.contains(", true)"),
        "the keyword's VALUE must land positionally:\n{src}"
    );
    assert!(
        !src.contains("upcase: true"),
        "no keyword may survive against a positional param:\n{src}"
    );
}

/// A call that already passes it positionally is untouched.
#[test]
fn an_already_positional_call_is_unchanged() {
    let src = emit(NAMED_KWARG, "<%= label_for(@rooms.first, true) %>\n");
    assert!(src.contains(", true)"), "{src}");
}

/// A `**attributes` helper keeps its hash: `id:` matches no parameter
/// name, so the trailing keywords still mean one Hash argument. This is
/// the case the fix must NOT break — it is why the rule is names rather
/// than "move every trailing keyword".
#[test]
fn a_splat_helper_keeps_its_hash() {
    let splat = "module RoomsHelper\n  def wrap(room, **attributes)\n    attributes.merge(name: room.name)\n  end\nend\n";
    let src = emit(splat, "<%= wrap(@rooms.first, id: \"x\") %>\n");
    assert!(
        src.contains("id:"),
        "the trailing keywords must stay a hash:\n{src}"
    );
}

/// A key naming no parameter at all declines — the call means something
/// this pass cannot prove.
#[test]
fn an_unknown_key_declines() {
    let src = emit(NAMED_KWARG, "<%= label_for(@rooms.first, bogus: 1) %>\n");
    assert!(src.contains("bogus:"), "left alone:\n{src}");
}
