//! Boolean columns hydrate as real booleans on the ruby-family emit.
//!
//! `<Model>Row.from_raw` wraps every adapter value in `ExprNode::Cast`
//! carrying the column's declared type, and the Ruby emitter's
//! `emit_cast` renders the narrowing (`.to_s` / `.to_i` / `.to_f`). It
//! had no `Bool` arm, so a boolean column was assigned RAW.
//!
//! That is invisible under CRuby — the gem shim yields `0`, and the
//! model's own `is_admin?` (`@is_admin == true || @is_admin == 1`)
//! compares it correctly. Under spinel it is not: `Db.column_value` is
//! `sqlite3_column_text`, so the value arrives as the STRING `"0"`,
//! which lands in a slot the RBS pins `bool` and reads as `true`.
//! lobsters' `Rack::MiniProfiler.authorize_request if @user &&
//! @user.is_admin?` then fired for a non-admin and took the request —
//! and, before the dispatch rescue, the whole process — down.
//!
//! Unlike the other coercions this one runs unconditionally: `to_s` on
//! a String is a semantic no-op, so it can be skipped for a
//! known-narrow value, but identity on a boolean is the bug itself.

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emitted_row_class(schema: &str) -> String {
    let tree = vec![
        (std::path::PathBuf::from("db/schema.rb"), schema.as_bytes().to_vec()),
        (
            std::path::PathBuf::from("app/models/user.rb"),
            b"class User < ApplicationRecord\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::analyze::Analyzer::new(&app).analyze(&mut app);
    let files = ruby::emit_lowered_models(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().contains("user_row"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no user_row emitted; got: {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

const NULLABLE: &str = r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "users", force: :cascade do |t|
    t.string "username"
    t.boolean "is_admin"
    t.integer "karma", null: false
  end
end
"#;

const NON_NULLABLE: &str = r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "users", force: :cascade do |t|
    t.boolean "is_admin", null: false
  end
end
"#;

#[test]
fn a_nullable_boolean_column_coerces_rather_than_passing_the_raw_value() {
    let src = emitted_row_class(NULLABLE);
    let line = src
        .lines()
        .find(|l| l.contains("is_admin") && l.contains("include?"))
        .unwrap_or_else(|| panic!("no coercion for is_admin:\n{src}"));
    // The false spellings every adapter can produce.
    assert!(line.contains("\"0\""), "got: {line}");
    assert!(line.contains("\"false\""), "got: {line}");
    // NULL is preserved by the surrounding guard, not by this cast.
    assert!(
        src.contains("row[\"is_admin\"].nil?"),
        "nullable column keeps its nil guard:\n{src}"
    );
}

#[test]
fn a_non_nullable_boolean_column_coerces_too() {
    let src = emitted_row_class(NON_NULLABLE);
    assert!(
        src.lines().any(|l| l.contains("is_admin") && l.contains("include?")),
        "non-nullable boolean must coerce as well:\n{src}"
    );
}

#[test]
fn non_boolean_columns_keep_their_own_coercions() {
    // Guard against the Bool arm swallowing neighbours.
    let src = emitted_row_class(NULLABLE);
    assert!(
        src.contains("(row[\"karma\"]).to_i"),
        "integer column should still use to_i:\n{src}"
    );
}
