//! Sends the analyzer types for every target but only the Ruby family can
//! run: `Array#fetch`, `each_slice`, `each_cons`, an enumerator's
//! `with_object`, `Array[...]`, `Hash[...]`, `include?` on a `Class` value
//! and Kernel's `Rational(...)` (what a rational literal ingests as).
//!
//! The other emitters render each as the method name on its receiver,
//! which nothing there defines, so every one refuses the construct by name
//! instead of exiting 0 with a call that cannot run. The Ruby family emits
//! them and builds (the runs are in `array_fetch_runs`,
//! `array_batches_typing`, `container_index_constructor`,
//! `include_query_receivers` and `rational_literal`).

use std::process::Command;

const MODEL: &str = r#"class Article < ApplicationRecord
  #: (Array[Integer]) -> Integer
  def self.first_plus(nums)
    nums.fetch(0).succ
  end

  #: (Array[Integer]) -> Integer
  def self.slices(nums)
    nums.each_slice(2).to_a.length
  end

  #: (Array[Integer]) -> Integer
  def self.pairs(nums)
    nums.each_cons(2).to_a.length
  end

  #: (Array[Integer]) -> Array[Integer]
  def self.memo(nums)
    nums.each_cons(2).with_object([]) { |pair, seen| seen << pair.first }
  end

  #: () -> Array[Integer]
  def self.built
    Array[1, 2]
  end

  #: () -> Hash[String, Integer]
  def self.table
    Hash[[["a", 1]]]
  end

  #: () -> Rational
  def self.third
    1/3r
  end

  #: (Class) -> bool
  def self.tagged?(klass) = klass.include?(Tagged)
end
"#;

const REFUSED: [&str; 8] = [
    "Array#fetch",
    "Array#each_slice",
    "Array#each_cons",
    "Enumerator#with_object",
    "Array[]",
    "Hash[]",
    "Kernel#Rational",
    "Module#include?",
];

fn build(root: &std::path::Path, target: &str) -> (bool, String) {
    let out = root.join(format!("out-{target}"));
    let output = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", target])
        .arg(root)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    (output.status.success(), String::from_utf8_lossy(&output.stderr).into_owned())
}

#[test]
fn only_the_ruby_family_emits_the_ruby_only_sends() {
    let root = std::env::temp_dir().join(format!("rh_ruby_only_sends_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in [
        ("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"articles\" do |t|\n    t.string \"title\"\n  end\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
        ("app/models/tagged.rb", "module Tagged\n  def tagged = true\nend\n"),
        ("app/models/article.rb", MODEL),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    for target in ["rust", "typescript", "go", "crystal", "python", "elixir"] {
        let (ok, text) = build(&root, target);
        assert!(!ok, "{target}: {text}");
        for construct in REFUSED {
            assert!(
                text.contains(&format!("error[unsupported]: {construct} not supported ({target})")),
                "{target} must refuse {construct}: {text}"
            );
        }
    }
    for target in ["ruby", "jruby", "spinel"] {
        let (ok, text) = build(&root, target);
        assert!(ok, "{target}: {text}");
        assert!(!text.contains("only the Ruby targets implement it"), "{target}: {text}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}
