//! Two jbuilder value routes campfire's `messages/_message.json.jbuilder`
//! and `users/_user.json.jbuilder` need, both found by the bot API's
//! `index includes message details` test reading the JSON back:
//!
//! * `json.created_at message.created_at.utc` — a temporal column of the
//!   template's argument reached through `utc`. `encode_value` rendered
//!   `Time#to_s` ("2026-09-12 13:26:25 UTC") where Rails renders
//!   `xmlschema(3)`; it now takes the `encode_datetime(<col>_raw)` route
//!   the Extract arm already gives `json.(message, :created_at)`.
//! * `json.(user, :role)` on an integer-backed `enum` — Rails serializes
//!   the LABEL ("bot"), the column reader answers the integer (2).

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby::emit_method;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_jbuilder_to_library_classes;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
    t.integer "role", default: 0, null: false
    t.datetime "created_at", null: false
  end
end
"#;

fn user_json() -> String {
    let files = tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/user.rb",
            "class User < ApplicationRecord\n  enum :role, %i[ member administrator bot ]\nend\n",
        ),
        (
            "app/views/users/_user.json.jbuilder",
            "json.(user, :id, :name, :role)\njson.joined_at user.created_at.utc\njson.plain user.created_at\n",
        ),
    ]);
    let app = ingest_app_from_tree(files).expect("ingest");
    let lcs = lower_jbuilder_to_library_classes(&app.views, &app, Vec::new());
    let m = lcs
        .iter()
        .flat_map(|l| l.methods.iter())
        .find(|m| m.name.as_str() == "user_json")
        .expect("user_json");
    emit_method(m)
}

#[test]
fn an_enum_attribute_encodes_as_its_label() {
    let body = user_json();
    assert!(
        body.contains(r#"JsonBuilder.encode_enum(["member", "administrator", "bot"], user.role)"#),
        "role should go through encode_enum with the declaration's labels:\n{body}"
    );
    assert!(body.contains("JsonBuilder.encode_value(user.name)"), "name stays a plain value:\n{body}");
}

#[test]
fn a_temporal_column_pair_encodes_from_its_stored_text() {
    let body = user_json();
    assert!(
        body.contains("JsonBuilder.encode_datetime(user.created_at_raw)"),
        "both `created_at.utc` and bare `created_at` should route to encode_datetime on the raw reader:\n{body}"
    );
    assert!(
        !body.contains("encode_value(user.created_at"),
        "no temporal column should reach encode_value:\n{body}"
    );
}
