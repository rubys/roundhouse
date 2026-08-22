//! `where(status: [ :active, :banned ])` — an enum column named by
//! SEVERAL labels at once.
//!
//! `lower::enum_symbols` translated a bare label (`where(status:
//! :active)` → `where(status: 0)`) and stopped there. Rails casts each
//! element of an array value through the same enum type and renders
//! `IN (0, 2)`; without the array arm the symbols reached the SQL
//! verbatim, the condition matched nothing, and the failure was SILENT
//! — campfire's account page asks for exactly this set and rendered an
//! empty user list, which reads as a missing feature rather than a
//! wrong query.

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

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
    t.integer "status", default: 0, null: false
  end
end
"#;

const MODEL: &str = r#"class User < ApplicationRecord
  enum :status, %i[ active deactivated banned ], default: :active
end
"#;

const CONTROLLER: &str = r#"class UsersController < ApplicationController
  def index
    @visible = User.where(status: [ :active, :banned ])
    @gone = User.where(status: :deactivated)
    @tagged = User.where(name: [ :active, :banned ])
  end
end
"#;

fn controller_src() -> String {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/user.rb", MODEL),
        ("app/controllers/users_controller.rb", CONTROLLER),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_controllers(&app)
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/controllers/users_controller.rb"))
        .map(|f| f.content.clone())
        .expect("emitted controller")
}

fn line_containing(src: &str, needle: &str) -> String {
    src.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line contains {needle:?}:\n{src}"))
        .to_string()
}

#[test]
fn every_label_in_an_array_value_maps_to_its_stored_integer() {
    let src = controller_src();
    let line = line_containing(&src, "@visible");
    assert!(
        line.contains("[ 0, 2 ]") || line.contains("[0, 2]"),
        "the array's labels map element-wise:\n{line}"
    );
    assert!(
        !line.contains(":active") && !line.contains(":banned"),
        "no enum LABEL survives into the query:\n{line}"
    );
}

/// The scalar arm this grew out of still holds — the array case is an
/// addition to `rewrite_value`, not a replacement.
#[test]
fn the_scalar_label_still_maps() {
    let src = controller_src();
    let line = line_containing(&src, "@gone");
    assert!(
        line.contains("status: 1") && !line.contains(":deactivated"),
        "the bare label still maps:\n{line}"
    );
}

/// The narrowing is by COLUMN: `name` is a String column no model
/// declares an enum on, so symbols under it are somebody else's
/// keyword and ride through untouched.
#[test]
fn a_non_enum_column_keeps_its_symbols() {
    let line = line_containing(&controller_src(), "@tagged");
    assert!(
        line.contains(":active") && line.contains(":banned"),
        "a non-enum column must not be mapped through the enum table:\n{line}"
    );
}
