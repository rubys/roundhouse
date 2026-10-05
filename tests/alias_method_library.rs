//! `alias_method :new, :old` in a library class body.
//!
//! Ruby copies `old` as it stands into `new`. Dropped at ingest, every
//! call to the alias was a NoMethodError; Shopify core has dozens
//! (`alias_method :eql?, :==` on value objects, `alias_method
//! :connection, :lease_connection`). A copy of the def already walked is
//! exact, including a later redefinition of the original not reaching
//! the alias.

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

fn emitted(lib_src: &str) -> String {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"posts\", force: :cascade do |t|\n    t.string \"body\", null: false\n  end\nend\n",
        ),
        ("app/models/post.rb", "class Post < ApplicationRecord\nend\n"),
        ("app/models/point.rb", lib_src),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_library(&app)
        .iter()
        .find(|f| f.path.ends_with("point.rb"))
        .expect("no point.rb emitted")
        .content
        .clone()
}

#[test]
fn alias_method_copies_the_instance_method_under_the_new_name() {
    let src = emitted(
        r#"class Point
  attr_reader :x

  def initialize(x)
    @x = x
  end

  def ==(other)
    x == other.x
  end
  alias_method :eql?, :==
end
"#,
    );
    assert!(src.contains("def ==(other)"), "{src}");
    assert!(src.contains("def eql?(other)"), "{src}");
    assert!(!src.contains("alias_method"), "{src}");
}

/// Inside `class << self` the alias is a class method, copied from the
/// class-side original.
#[test]
fn alias_keyword_copies_a_defined_method_in_a_class_and_module() {
    for source in [
        "class Probe\n  def display_name\n    \"shown\"\n  end\n  alias translated_fault_name display_name\n  def old_name?\n    true\n  end\n  alias next_name old_name?\nend\n",
        "module Probe\n  def display_name\n    \"shown\"\n  end\n  alias translated_fault_name display_name\nend\n",
    ] {
        let classes = roundhouse::ingest::ingest_library_classes(source.as_bytes(), "probe.rb").expect(source);
        let names: Vec<_> = classes[0].methods.iter().map(|method| method.name.as_str().to_string()).collect();
        assert!(names.iter().any(|name| name == "translated_fault_name"), "{names:?}");
        if source.contains("old_name?") {
            assert!(names.iter().any(|name| name == "next_name"), "{names:?}");
        }
    }
    let model = "class FaultAlarm < ApplicationRecord\n  def display_name\n    \"shown\"\n  end\n  alias translated_fault_name display_name\nend\n";
    let ingested = roundhouse::ingest::ingest_model(
        model.as_bytes(),
        "app/models/fault_alarm.rb",
        &roundhouse::schema::Schema::default(),
        &Default::default(),
    ).expect("model alias").expect("model");
    assert!(ingested.methods().any(|method| method.name.as_str() == "translated_fault_name"));
    let err = roundhouse::ingest::ingest_library_classes(
        b"class Probe\n  alias missing gone\nend\n",
        "probe.rb",
    ).expect_err("undefined alias");
    assert!(err.to_string().contains("alias names a method"));
}

#[test]
fn alias_method_in_the_singleton_class_copies_the_class_method() {
    let src = emitted(
        r#"class Point
  class << self
    def origin
      new
    end
    alias_method :zero, :origin
  end
end
"#,
    );
    assert!(src.contains("def self.origin"), "{src}");
    assert!(src.contains("def self.zero"), "{src}");
}
