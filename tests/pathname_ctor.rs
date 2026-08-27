//! `Pathname(p)` is Kernel's conversion FUNCTION — it emits as
//! `Pathname.new(p)`.
//!
//! Spinel's bundled `pathname` package says why, in its own header: the
//! mixed-in `Kernel#Pathname()` "cannot be spelled in Spinel yet (a
//! toplevel method named after a class collides with the class's own
//! symbol) -- use Pathname.new". campfire's
//! `CableHelper.script_aware_action_cable_meta_tag` builds the Action
//! Cable URL out of two of them, and the layout renders it into the
//! `<head>` of every page, so the bare call stopped the build on a line
//! that runs on every request.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "rooms", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

fn emitted(helper: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/room.rb", "class Room < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/helpers/cable_helper.rb", helper),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :rooms\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    // A post-analyze lowering: emitting straight off the ingest would
    // never run it.
    let _ = roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("cable_helper.rb"))
        .map(|f| f.content.clone())
        .expect("cable_helper.rb")
}

#[test]
fn the_conversion_function_becomes_the_constructor() {
    let src = emitted(
        "module CableHelper\n  def cable_url(script_name)\n    \
         (Pathname(script_name) + Pathname(\"/cable\")).to_s\n  end\nend\n",
    );
    assert!(
        src.contains("Pathname.new(script_name)") && src.contains("Pathname.new(\"/cable\")"),
        "both conversions must emit as constructor calls:\n{src}"
    );
    assert!(
        !src.contains("Pathname(script_name)"),
        "no bare Kernel conversion may survive — nothing defines it:\n{src}"
    );
}

/// The CLASS is untouched: `Pathname.new` was already the spelling, and
/// a rewrite that fired on it would nest constructors.
#[test]
fn an_existing_constructor_call_is_left_alone() {
    let src = emitted(
        "module CableHelper\n  def cable_url(script_name)\n    \
         Pathname.new(script_name).to_s\n  end\nend\n",
    );
    assert!(
        src.contains("Pathname.new(script_name).to_s") && !src.contains("Pathname.new(Pathname"),
        "an explicit constructor stays exactly one constructor:\n{src}"
    );
}
