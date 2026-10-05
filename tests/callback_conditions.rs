//! `if:` / `unless:` on a model callback. Ingest used to reject any
//! callback carrying one, so the callback was dropped entirely: a
//! `before_validation :assign_color, on: :create, if: -> { color.blank? }`
//! never ran, the column stayed blank, and the record failed validation.
//! A zero-arity lambda body is spliced into a guard (Rails
//! `instance_exec`s it with `self` the record); a Symbol is the predicate
//! method called on the record.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :color\n    t.string :name\n  end\nend\n";

const WIDGET: &str = r##"class Widget < ApplicationRecord
  before_validation :assign_color, on: :create, if: -> { color.blank? }
  before_save :shout, unless: :loud?
  before_destroy :noop, if: :frozen?

  private
    def assign_color
      self.color = "#000000"
    end

    def loud?
      name == "LOUD"
    end

    def shout
      self.name = name.to_s.upcase
    end

    def frozen?
      false
    end
end
"##;

fn emitted() -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/widget.rb", WIDGET),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.ends_with("widget.rb"))
        .expect("widget.rb emitted")
        .content
        .clone()
}

#[test]
fn a_lambda_if_condition_guards_the_callback() {
    let src = emitted();
    assert!(
        src.contains("assign_color"),
        "the callback must not be dropped:\n{src}"
    );
    assert!(
        src.contains("ActiveSupport.blank?(self.color)"),
        "the lambda condition must guard the callback:\n{src}"
    );
}

#[test]
fn a_symbol_unless_condition_negates_the_predicate() {
    let src = emitted();
    // `before_save :shout, unless: :loud?` → a negated guard around the
    // call (the method names alone appear as definitions on main too).
    assert!(src.contains("if !(loud?)"), "the unless guard must negate the predicate:\n{src}");
    // `before_destroy :noop, if: :frozen?` → guarded by the predicate.
    assert!(src.contains("if frozen?"), "{src}");
}

/// A lambda/proc condition with parameters, several statements, a
/// control-flow escape or a local write is NOT spliced: inline in the
/// hook it would read an unbound `r`/`p`, `return` out of the whole
/// callback chain, or leak a local. Those decline to the
/// unsupported-DSL warning, as before conditions were modelled — the
/// callback is dropped, never emitted with a broken guard.
const DECLINED: &str = r##"class Widget < ApplicationRecord
  before_save :a1, if: ->(r) { r.name.present? }
  before_save :a2, if: proc { |p| p.name.present? }
  before_save :a3, if: -> { return false if name.nil?; true }
  before_save :a4, if: -> { n = name; n.present? }
  before_save :a6, if: -> { (a, b = name, 1) && a }
  before_save :a7, if: -> { name in String => t }
  before_save :a8, if: -> { begin; name.present?; rescue => e; false; end }
  before_save :a9, if: -> { (@a, @b = name, 1) && @a }
  before_save :a5, if: -> { name.present? }

  private
    def a1; self.color = "1"; end
    def a2; self.color = "2"; end
    def a3; self.color = "3"; end
    def a4; self.color = "4"; end
    def a6; self.color = "6"; end
    def a7; self.color = "7"; end
    def a8; self.color = "8"; end
    def a9; self.color = "9"; end
    def a5; self.color = "5"; end
end
"##;

fn emitted_from(widget: &str) -> String {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        ("app/models/widget.rb", widget),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    roundhouse::emit::ruby::emit_lowered_models(&app)
        .into_iter()
        .find(|f| f.path.ends_with("widget.rb"))
        .expect("widget.rb emitted")
        .content
        .clone()
}

#[test]
fn only_a_zero_parameter_single_expression_condition_is_spliced() {
    let src = emitted_from(DECLINED);
    // Never an unbound parameter in a guard.
    assert!(!src.contains("r.name") && !src.contains("p.name"), "unbound param spliced:\n{src}");
    // The declined callbacks are dropped (defined, never called from the hook).
    let hook = src.split("def before_save").nth(1).expect("before_save hook").split("\n  end").next().unwrap();
    for m in ["a1", "a2", "a3", "a4", "a6", "a7", "a8", "a9"] {
        assert!(!hook.contains(m), "{m} must decline, not run with a broken guard:\n{hook}");
    }
    // The zero-parameter, single-expression one is guarded.
    assert!(hook.contains("if ActiveSupport.present?(self.name)") && hook.contains("a5"), "{hook}");
}
