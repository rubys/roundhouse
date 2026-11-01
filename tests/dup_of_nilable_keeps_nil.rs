//! `nil.dup` is `nil`, so the copy of a `T?` is a `T?`. Dispatch over a
//! union receiver skipped the Nil arm and typed `x.dup` as just the
//! copy of `T`, which made a large app's
//!
//! ```ruby
//! #: [String?, String]?
//! attr_reader :status_change
//! # initialize: @status_change = status_change.dup.freeze
//! ```
//!
//! read back as non-nil: the reader's type lost its `?`, `present?` on
//! it grounded to `!x.empty?`, and every unit test whose changeset had
//! no status change raised `undefined method 'empty?' for nil`
//! (`UpdateDiscoveryDataForConditionChangeset#should_trigger_update?`).

use std::path::PathBuf;

const FILES: &[(&str, &str)] = &[
    ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    (
        "app/models/chg/changeset.rb",
        r#"module Chg
  class Changeset
    #: (?status_change: [String?, String]?, ?tags: Array[String]?) -> void
    def initialize(status_change: nil, tags: nil)
      @status_change = status_change.dup.freeze
      @tags = tags.clone
    end

    #: [String?, String]?
    attr_reader :status_change

    #: Array[String]?
    attr_reader :tags

    #: -> bool
    def status_changed? = status_change.present?

    #: -> bool
    def tagged? = tags.present?
  end
end
"#,
    ),
];

fn emit() -> (String, String) {
    let root: PathBuf =
        std::env::temp_dir().join(format!("rh_dup_nilable_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for (path, text) in FILES {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let out = root.with_extension("out");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "ruby"])
        .arg(&root)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    let read = |path: &str| std::fs::read_to_string(out.join(path)).unwrap_or_default();
    let (ruby, sig) = (
        read("app/models/chg/changeset.rb"),
        read("sig/app/models/chg/changeset.rbs"),
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&out);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    (ruby, sig)
}

#[test]
fn the_copy_of_a_nilable_stays_nilable() {
    let (ruby, sig) = emit();
    assert!(sig.contains("attr_reader status_change: Array[String?]?"), "{sig}");
    assert!(sig.contains("attr_reader tags: Array[String]?"), "{sig}");
    // Nilable receivers: `empty?` must sit behind a nil guard.
    for name in ["status_change", "tags"] {
        let guarded = format!("!({name}.nil?) && !({name}.empty?)");
        assert!(ruby.contains(&guarded), "{name}.present? lost its nil guard:\n{ruby}");
    }
}
