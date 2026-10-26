//! A plain (non-model) class's body call to its own `def self.` macro
//! (#1281). Ruby runs the macro as the class loads, so the methods it
//! defines exist on the class. The call was replayed unexpanded, which
//! the Spinel target refuses (`define_method` with a non-literal name).
//! It now expands as a model's class macro does: a `define_method(name) {
//! … }`, or `define_method(name, Mod.instance_method(name))` for an app
//! module that defines it, becomes the class's own method, and a call
//! that can't expand is reported once, at the call, not replayed.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n";

/// The macro copies a module's method in through `instance_method`.
const BORROWED: &str = r##"module Gadget
  def label
    "gadget #{name}"
  end

  class Sprocket
    def self.borrow(mod, *names)
      names.each do |meth|
        define_method(meth, mod.instance_method(meth))
      end
    end

    attr_reader :name

    def initialize(name)
      @name = name
    end

    def describe
      label
    end

    borrow(Gadget, :label)
  end
end
"##;

/// The macro defines each name from a block.
const BLOCKS: &str = r##"module Gadget
  class Sprocket
    def self.borrow(*names)
      names.each do |meth|
        define_method(meth) { "gadget #{meth}" }
      end
    end

    borrow(:label, :title)
  end
end
"##;

fn app(gadget: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("lib/gadget.rb", gadget)
}

#[test]
fn a_borrowed_method_runs_on_cruby() {
    app(BORROWED)
        .run_ruby("got = Gadget::Sprocket.new(\"x\").describe\nraise got.inspect unless got == \"gadget x\"\nputs \"PASS\"\n")
        .assert_passes();
}

#[test]
fn block_definitions_run_on_cruby() {
    app(BLOCKS)
        .run_ruby("s = Gadget::Sprocket.new\ngot = [s.label, s.title]\nraise got.inspect unless got == [\"gadget label\", \"gadget title\"]\nputs \"PASS\"\n")
        .assert_passes();
}

fn spinel_sprocket(gadget: &str) -> String {
    let (emitted, errors) = app(gadget).emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:#?}");
    let found = walk(&emitted).into_iter().find(|p| std::fs::read_to_string(p).is_ok_and(|s| s.contains("class Sprocket")));
    std::fs::read_to_string(found.expect("Sprocket emitted")).unwrap()
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else if path.extension().is_some_and(|e| e == "rb") {
            out.push(path);
        }
    }
    out
}

#[test]
fn the_spinel_emit_defines_the_methods_and_drops_the_call() {
    let borrowed = spinel_sprocket(BORROWED);
    assert!(borrowed.contains("def label"), "{borrowed}");
    assert!(!borrowed.contains("borrow(Gadget"), "{borrowed}");
    let blocks = spinel_sprocket(BLOCKS);
    assert!(blocks.contains("def label") && blocks.contains("def title"), "{blocks}");
    assert!(!blocks.contains("borrow(:label"), "{blocks}");
}

/// A module the app doesn't define, or a method it doesn't have: the call
/// can't expand, and is reported once, where it is written.
#[test]
fn a_call_that_cannot_expand_is_reported_at_the_call() {
    for gadget in [BORROWED.replace("borrow(Gadget, :label)", "borrow(Kernel, :label)"), BORROWED.replace("borrow(Gadget, :label)", "borrow(Gadget, :missing)")] {
        let warnings = app(&gadget).check_errors();
        let reports: Vec<&String> = warnings.iter().filter(|w| w.contains("`borrow` defines methods this call does not expand")).collect();
        assert_eq!(reports.len(), 1, "{warnings:#?}");
    }
}
