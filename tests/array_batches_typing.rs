//! `each_slice(n)` / `each_cons(n)` yield same-element sub-arrays.
//!
//! Records and ids are batched through them, often chaining the
//! enumerator (`missing.each_slice(500).flat_map { |batch| … }` or
//! `each_cons(2).with_object([])`). The analyzer knew none of these, so
//! every such site read as `no known method`.

use std::process::Command;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn check(name: &str, body: &str) -> String {
    let root = std::env::temp_dir().join(format!("rh_array_batches_typing_{}_{name}", std::process::id()));
    let files = [
        ("roundhouse.yml", "test_paths:\n  - test\n".to_string()),
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :accounts do |t|\n    t.string :name\n  end\nend\n".to_string()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
        ("app/models/account.rb", format!("class Account < ApplicationRecord\n  #: (Array[Integer]) -> untyped\n  def use(ids)\n{body}\n  end\nend\n")),
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
fn the_block_takes_a_batch() {
    let text = check("block", "    ids.each_slice(2) { |batch| batch.zzz }\n    ids.each_cons(2) { |pair| pair.yyy }");
    assert!(text.contains("no known method `zzz` on Array[Integer]"), "{text}");
    assert!(text.contains("no known method `yyy` on Array[Integer]"), "{text}");
    assert!(!text.contains("no known method `each_slice`"), "{text}");
    assert!(!text.contains("no known method `each_cons`"), "{text}");
}

#[test]
fn a_chained_enumerator_yields_batches() {
    let text = check("chain", "    ids.each_slice(2).flat_map { |batch| batch.xxx }");
    assert!(text.contains("no known method `xxx` on Array[Integer]"), "{text}");
    assert!(!text.contains("no known method `each_slice`"), "{text}");
}

#[test]
fn the_enumerator_folds_with_an_object() {
    let text = check("with_object", "    ids.each_cons(2).with_object([]) { |pair, acc| pair.www }");
    assert!(text.contains("no known method `www` on Array[Integer]"), "{text}");
    assert!(!text.contains("no known method `with_object`"), "{text}");
}

const BATCHES: &str = r#"class Batches
  #: (Array[Integer]) -> Array[Integer]
  def sums(ids)
    ids.each_slice(2).flat_map { |batch| [batch.sum] }
  end

  #: (Array[Integer]) -> Array[Array[Integer]]
  def windows(ids)
    ids.each_cons(2).with_object([]) { |pair, acc| acc << pair }
  end
end
"#;

const SCRIPT: &str = r#"
require_relative "app/models/batches"
batches = Batches.new
raise "slices: #{batches.sums([1, 2, 3, 4, 5]).inspect}" unless batches.sums([1, 2, 3, 4, 5]) == [3, 7, 5]
raise "windows: #{batches.windows([1, 2, 3]).inspect}" unless batches.windows([1, 2, 3]) == [[1, 2], [2, 3]]
puts "array batches contract passed"
"#;

#[test]
fn batches_run_in_emitted_ruby() {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("app/services/batches.rb", BATCHES)
        .run_ruby(SCRIPT)
        .assert_passes();
}
