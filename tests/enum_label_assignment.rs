//! A Rails ENUM column assigned by its LABEL at RUNTIME.
//!
//! `enum :role, %i[member administrator bot]` lets `user.role =
//! "administrator"` store `1`. `lower::enum_symbols` already translates
//! the labels an app writes down (`where(role: :bot)`); it cannot see a
//! label that only exists at runtime, and campfire's
//! `Accounts::UsersController` reads one off the request:
//!
//! ```ruby
//! { role: params.require(:user)[:role].presence_in(%w[ member administrator ]) || "member" }
//! ```
//!
//! The synthesized writers cast to the column's slot type, so that
//! became `"administrator".to_i` — **zero**, which is `member`. A role
//! change silently demoted the user instead of failing.
//!
//! Three writers take an untyped value and so all three need the
//! mapping: `initialize`, `update`/`update!`, and `[]=`. A column that
//! is not an enum keeps its plain `Cast`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::emit::rust;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

fn app() -> roundhouse::App {
    ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
    t.integer "role", null: false, default: 0
    t.integer "rank", null: false, default: 0
  end
end
"#,
        ),
        (
            "app/models/user.rb",
            r#"class User < ApplicationRecord
  enum :role, %i[ member administrator bot ]
end
"#,
        ),
    ]))
    .expect("ingest enum app")
}

fn user_src() -> String {
    let files = ruby::emit_lowered_models(&app());
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("user.rb"))
        .map(|f| f.content.clone())
        .expect("user.rb")
}

fn line_containing(src: &str, needle: &str) -> String {
    src.lines()
        .find(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?}:\n{src}"))
        .trim()
        .to_string()
}

/// The mapping travels as the declaration's two parallel arrays.
#[test]
fn the_enum_columns_writers_map_the_label() {
    let src = user_src();
    for (site, needle) in [
        (
            "update",
            "self.role = ActiveRecord.enum_int((attrs[:role]).to_s,",
        ),
        ("[]=", "@role = ActiveRecord.enum_int((value).to_s,"),
    ] {
        assert!(
            src.contains(needle),
            "the {site} writer must map the label:\n{src}"
        );
    }
    assert!(
        src.contains(r#"["member", "administrator", "bot"], [0, 1, 2]"#),
        "the declaration's labels and stored values ride along:\n{src}"
    );
}

/// Schema defaults are stored values. Only supplied attributes pass
/// through the enum writer, including in a child with a different map.
#[test]
fn initialize_stores_the_default_and_maps_explicit_input() {
    let src = user_src();
    assert!(src.contains("@role = 0"), "raw schema default:\n{src}");
    let line = line_containing(&src, "self.role = ActiveRecord.enum_int");
    assert!(
        line.contains("(attrs[:role]).to_s") && line.ends_with("if attrs.key?(:role)"),
        "only explicit input must pass through the mapping:\n{line}"
    );
}

/// A plain integer column beside it is untouched — the mapping is
/// keyed by the enum declaration, not by the column type.
#[test]
fn a_non_enum_integer_column_keeps_its_cast() {
    let src = user_src();
    assert!(
        !line_containing(&src, "self.rank = ").contains("enum_int"),
        "rank is not an enum:\n{src}"
    );
}

#[test]
fn rust_enum_runtime_calls_own_string_array_literals() {
    let mut app = app();
    roundhouse::session::analyze_and_lower(&mut app);
    let source = rust::emit(&app)
        .into_iter()
        .find(|file| file.path.to_string_lossy().ends_with("models/user.rs"))
        .expect("Rust User model")
        .content;

    assert!(
        source.contains("vec![(\"member\").to_string()"),
        "enum helper Array[String] arguments must emit owned Rust strings:\n{source}"
    );
}
