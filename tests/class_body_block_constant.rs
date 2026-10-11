//! A constant written in a block passed to a class-body call.
//!
//! A class declares its options in
//! `setup do |t| LABEL = t.option(…) end`. Ruby puts the
//! constant in the class (the block's cref), as Sorbet reads it; every read,
//! bare in the class or qualified elsewhere, was refused as `constant not
//! supported`, though the emitted body ran the block and defined it.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const BASE: &str = r#"module Outer
  class Base
    #: () { (Base) -> void } -> void
    def self.setup(&block) = block.call(new)

    #: (String) -> String
    def option(name) = name
  end
end
"#;

const HANDLER: &str = r#"module Outer
  class Handler < Base
    setup do |t|
      LABEL = t.option("run.opt") #: String
    end

    #: -> String
    def self.label = LABEL.upcase
  end
end
"#;

const READER: &str = r#"module Outer
  class Reader
    #: -> String
    def self.label = Handler::LABEL
  end
end
"#;

#[test]
fn a_class_body_block_constant_types_and_runs() {
    let run = emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("app/models/outer/base.rb", BASE)
        .write("app/models/outer/handler.rb", HANDLER)
        .write("app/models/outer/reader.rb", READER)
        .run_ruby("puts Outer::Handler.label\nputs Outer::Reader.label\n");
    run.assert_passes();
    assert_eq!(run.stdout, "RUN.OPT\nrun.opt\n");
}

#[test]
fn a_constant_no_block_writes_stays_refused() {
    use std::collections::HashMap;
    use std::path::PathBuf;
    let missing = "module Outer\n  class Other\n    #: -> String\n    def self.label = Handler::Missing\n  end\nend\n";
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\nend\n"),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("app/models/outer/base.rb", BASE),
        ("app/models/outer/handler.rb", HANDLER),
        ("app/models/outer/other.rb", missing),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    roundhouse::session::analyze_and_lower(&mut app);
    let errors: Vec<String> = roundhouse::analyze::diagnose(&app).into_iter().map(|d| d.to_string()).collect();
    assert!(errors.iter().any(|e| e.contains("Missing")), "{errors:?}");
    assert!(!errors.iter().any(|e| e.contains("LABEL")), "{errors:?}");
}
