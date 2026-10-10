//! An include of a module that only nests other declarations.
//!
//! A Zeitwerk implicit namespace is a directory of classes with no file of
//! its own (`app/models/outer/inner/leaf/` holding `item.rb`).
//! A class that does `include Inner::Leaf` reads the nested classes bare.
//! The include was refused as `includes unresolved` (no class models the
//! module), though the bare read already resolved through it.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ITEM: &str = r#"module Outer
  module Inner
    module Leaf
      class Item
        def self.label = "item"
      end
    end
  end
end
"#;

const READER: &str = r#"module Outer
  module Other
    class Reader
      include Inner::Leaf

      def label = Item.label
    end
  end
end
"#;

const MISSING: &str = r#"module Outer
  class Broken
    include Inner::Nowhere
  end
end
"#;

#[test]
fn an_included_implicit_namespace_resolves_and_runs() {
    let run = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/outer/inner/leaf/item.rb", ITEM)
        .write("app/models/outer/other/reader.rb", READER)
        .run_ruby("puts Outer::Other::Reader.new.label\nputs Outer::Other::Reader.include?(Outer::Inner::Leaf)\n");
    run.assert_passes();
    assert_eq!(run.stdout, "item\ntrue\n");
    let reader = std::fs::read_to_string(run.emitted.join("app/models/outer/other/reader.rb")).expect("reader.rb");
    assert!(!reader.contains("include not supported"), "{reader}");
}

#[test]
fn an_include_naming_no_namespace_stays_refused() {
    use std::collections::HashMap;
    use std::path::PathBuf;
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/outer/inner/leaf/item.rb", ITEM),
        ("app/models/outer/broken.rb", MISSING),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<String> = roundhouse::analyze::diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    assert!(errors.iter().any(|e| e.contains("includes unresolved Inner::Nowhere")), "{errors:?}");
}
