//! A PostgreSQL array column (`t.bigint "tag_ids", array: true`) (#1272).
//! Rails reads it as an `Array` of the column's type: nil while a nullable
//! column is unset, `[]` from a `default: []`, each assigned element cast
//! (activerecord 8.1.4 on PostgreSQL). The schema ingest dropped the
//! option silently. The column now carries the flag, and each target that
//! stores rows itself, in SQLite, reports it as a gap: its storage holds
//! one element. The futamura and Roda targets read the database's own type
//! and report nothing, and neither does the ruby family, which stores the
//! Array. The ingested analysis keeps the element type; a target that
//! stores the Array retypes it before its analysis.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 1) do
  create_table \"members\", force: :cascade do |t|
    t.string \"name\"
    t.bigint \"tag_ids\", array: true
    t.string \"labels\", array: true, default: [], null: false
  end
end
";

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/member.rb", "class Member < ApplicationRecord\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
}

fn app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files().into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    ingest_app_from_tree(tree).expect("ingest")
}

#[test]
fn the_column_carries_array_true() {
    let app = app();
    let table = app.schema.tables.values().find(|t| t.name.as_str() == "members").expect("members");
    let array = |name: &str| table.columns.iter().find(|c| c.name.as_str() == name).expect(name).array;
    assert!(array("tag_ids") && array("labels"));
    assert!(!array("name"));
}

/// The analysis still types the column as its element: it feeds every
/// lowered target's method dispatch, and their storage holds one element
/// (`labels.first` would otherwise lower to an Array call on a String
/// field). The `Array` type comes with the storage that holds one.
#[test]
fn the_analysis_keeps_the_element_type_for_now() {
    let app = app();
    let member = app.models.iter().find(|m| m.name.0.as_str() == "Member").expect("Member");
    let field = |name: &str| member.attributes.fields.get(&roundhouse::Symbol::from(name)).cloned().expect(name);
    assert_eq!(field("tag_ids"), Ty::Union { variants: vec![Ty::Int, Ty::Nil].into() });
    assert_eq!(field("labels"), Ty::Str);
}

fn warnings(target: &str) -> Vec<String> {
    warnings_for(target, &files(), "base")
}

fn warnings_for(target: &str, files: &[(&str, &str)], tag: &str) -> Vec<String> {
    let root = std::env::temp_dir().join(format!("roundhouse-array-columns-{tag}-{target}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for &(path, body) in files {
        let file = root.join("app_src").join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target, "--allow-unsupported", "-o"])
        .arg(root.join("out"))
        .arg(root.join("app_src"))
        .output()
        .expect("run roundhouse");
    std::fs::remove_dir_all(&root).ok();
    String::from_utf8_lossy(&out.stderr).lines().filter(|l| l.contains("PostgreSQL array column")).map(String::from).collect()
}

#[test]
fn a_target_that_stores_rows_reports_each_array_column() {
    for target in ["rust", "typescript"] {
        let lines = warnings(target);
        assert_eq!(lines.len(), 2, "{target}: {lines:#?}");
        assert!(lines.iter().any(|l| l.contains("`tag_ids`")) && lines.iter().any(|l| l.contains("`labels`")), "{target}: {lines:#?}");
    }
}

/// The ruby family stores the Array (`lower::array_columns`,
/// `tests/array_column_storage.rs`), so it reports none.
#[test]
fn the_ruby_family_reports_no_array_column() {
    for target in ["ruby", "spinel", "jruby"] {
        let lines = warnings(target);
        assert!(lines.is_empty(), "{target}: {lines:#?}");
    }
}

#[test]
fn futamura_reports_no_array_column() {
    assert!(warnings("futamura").is_empty());
}

/// A view that projects an array column directly is an array column too:
/// it re-exposes the table's column, type and `array:` alike.
#[test]
fn a_view_projecting_an_array_column_carries_the_flag() {
    let schema = "ActiveRecord::Schema[8.1].define(version: 1) do
  create_table \"members\", force: :cascade do |t|
    t.string \"name\"
    t.bigint \"tag_ids\", array: true
  end
  create_view \"member_tags\", sql_definition: <<-SQL
      SELECT members.name AS name, members.tag_ids AS tag_ids FROM members
  SQL
end
";
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", schema),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let app = ingest_app_from_tree(tree).expect("ingest");
    let view = app.schema.tables.values().find(|t| t.name.as_str() == "member_tags").expect("member_tags view");
    let column = |name: &str| view.columns.iter().find(|c| c.name.as_str() == name).expect(name);
    assert!(column("tag_ids").array, "{:?}", column("tag_ids"));
    assert!(!column("name").array);
}

/// The view's array column is reported where the table's is.
#[test]
fn a_view_over_an_array_column_warns_too() {
    let schema = format!(
        "{}",
        SCHEMA.replace(
            "  end\nend\n",
            "  end\n  create_view \"member_tags\", sql_definition: <<-SQL\n      SELECT members.name AS name, members.tag_ids AS tag_ids FROM members\n  SQL\nend\n",
        )
    );
    let mut files = files();
    files[0] = ("db/schema.rb", schema.as_str());
    for target in ["rust", "typescript"] {
        let lines = warnings_for(target, &files, "view");
        assert!(lines.iter().any(|l| l.contains("member_tags") && l.contains("`tag_ids`")), "{target}: {lines:#?}");
        assert_eq!(lines.len(), 3, "the two table columns and the view's\n{target}: {lines:#?}");
    }
    assert!(warnings_for("ruby", &files, "view").is_empty());
}
