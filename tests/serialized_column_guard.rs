//! A guard on a column reader narrows with the reader's type.
//!
//! `return false if payload.nil?` on a model with `serialize :payload,
//! coder: JSON` narrowed the reads after it to the storage column's
//! `String`, so `payload.fetch(...)` was refused on String. The narrowing
//! now starts from the reader the class registered; a plain column keeps
//! its type.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table :things do |t|\n    t.string :title\n    t.text :payload\n  end\nend\n";

fn check(model: &str) -> (bool, String) {
    let root = std::env::temp_dir().join(format!("rh_serialized_column_guard_{}_{}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst)));
    let files = [
        ("roundhouse.yml", "test_paths:\n  - test\n"),
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/thing.rb", model),
    ];
    for (path, text) in &files {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
    std::fs::remove_dir_all(&root).unwrap();
    (output.status.success(), format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn a_nil_guard_on_a_serialized_column_keeps_the_decoded_type() {
    let model = "class Thing < ApplicationRecord\n  serialize :payload, coder: JSON\n\n  def flag?\n    return false if payload.nil?\n\n    payload.fetch(:flag, false)\n  end\nend\n";
    let (ok, text) = check(model);
    assert!(ok, "{text}");
}

#[test]
fn a_nil_guard_on_a_plain_column_narrows_to_the_column_type() {
    let model = "class Thing < ApplicationRecord\n  def flag?\n    return false if title.nil?\n\n    title.fetch(:flag, false)\n  end\nend\n";
    let (ok, text) = check(model);
    assert!(!ok, "{text}");
    assert!(text.contains("no known method `fetch` on String"), "{text}");
}

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// The guarded read runs against the decoded value, not the storage text.
#[test]
fn a_guarded_serialized_read_runs() {
    emit_and_run::real_blog()
        .edit(
            "app/models/article.rb",
            "class Article < ApplicationRecord\n",
            "class Article < ApplicationRecord\n  serialize :body, coder: JSON\n\n  #: -> untyped\n  def flag?\n    return false if body.nil?\n\n    body.fetch(\"flag\", false)\n  end\n\n",
        )
        .run_ruby(
            r#"article = Article.new(title: "Probe", body: { "flag" => true })
raise "decoded read lost" unless article.flag? == true
raise "nil guard lost" unless Article.new(title: "Probe").flag? == false
"#,
        )
        .assert_passes();
}
