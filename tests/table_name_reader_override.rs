//! A class-side `table_name` reader answering a literal binds the table.
//!
//! A `Catalog::Item::Detail` names its table with `class <<
//! self; def table_name; "catalog_item_details"; end; end`. Only
//! `self.table_name = …` bound the schema row, so the model read the
//! conventional `details`, had no columns, and every column reader
//! (`dimension_unit`, `weight_value`, …) was `no known method`.

use std::process::Command;

fn check(name: &str, body: &str) -> String {
    let root = std::env::temp_dir().join(format!("rh_table_name_reader_override_{}_{name}", std::process::id()));
    let files = [
        ("roundhouse.yml", "test_paths:\n  - test\n".to_string()),
        ("db/structure.sql", "CREATE TABLE public.item_details (\n    id bigint NOT NULL,\n    dimension_unit character varying NOT NULL\n);\n".to_string()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
        ("app/models/detail.rb", format!("class Detail < ApplicationRecord\n{body}\n  #: -> untyped\n  def use = dimension_unit.zzz\nend\n")),
    ];
    for (path, text) in files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

#[test]
fn a_literal_reader_binds_the_row() {
    for (name, body) in [
        ("singleton", "  class << self\n    #: -> String\n    def table_name\n      \"item_details\"\n    end\n  end\n"),
        ("endless", "  def self.table_name = \"item_details\"\n"),
    ] {
        let text = check(name, body);
        assert!(text.contains("no known method `zzz` on String"), "{name}: {text}");
        assert!(!text.contains("no known method `dimension_unit`"), "{name}: {text}");
    }
}

#[test]
fn a_computed_reader_keeps_the_convention() {
    let text = check("computed", "  def self.table_name = \"box_\" + \"details\"\n");
    assert!(text.contains("method call `dimension_unit` has unresolved type"), "{text}");
    assert!(!text.contains("on String"), "{text}");
}
