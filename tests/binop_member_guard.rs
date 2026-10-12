//! A guard on `x.attr` speaks for the later reads of `x.attr`.
//!
//! ```ruby
//! things.select { |t| t.score && t.score >= 0.5 }
//! ```
//!
//! `t.score` is a nullable column read. The right side of `&&` only runs when
//! the first read was truthy, and a column reader is plain state, so the second
//! read is the same non-nil value. Narrowing it is sound only while three
//! things hold, and each has a test here that a plausible wrong implementation
//! (narrowing every `x.attr`, or narrowing by name alone) would fail:
//!
//! * the attribute is a declared column/accessor, not a `def` the app wrote
//!   (a `def score` may return something different the second time),
//! * the local is not rebound, nor written through, between guard and use,
//! * an unguarded `x.attr >= 0.5` keeps its `Float? >= Float` error.

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};

fn binops(thing: &str) -> Vec<String> {
    binops_with(thing, &[])
}

fn binops_with(thing: &str, extra: &[(&str, &str)]) -> Vec<String> {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\nend\n",
        ),
        ("app/models/thing.rb", thing),
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define(version: 1) do
  create_table "things", force: :cascade do |t|
    t.float "score"
    t.float "other"
  end
end
"#,
        ),
    ]
    .iter()
    .chain(extra.iter())
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::IncompatibleBinop { .. }))
        .map(|d| format!("{:?}", d.kind))
        .collect()
}

fn thing_with(body: &str) -> String {
    format!("class Thing < ApplicationRecord\n{body}\nend\n")
}

#[test]
fn a_guarded_column_read_on_a_local_narrows() {
    let found = binops(&thing_with(
        r#"  def self.keep
    Thing.all.select { |t| t.score && t.score >= 0.5 }
  end
  def self.present_guard
    Thing.all.select { |t| t.score.present? && t.score > 1 }
  end
  def self.not_nil
    Thing.all.select { |t| !t.score.nil? && t.score > 1 }
  end
  def self.branch
    t = Thing.find(1)
    if t.score
      t.score + 1.5
    end
  end
  def self.ternary
    t = Thing.find(1)
    t.score ? t.score * 2 : 0
  end
  def self.negated
    t = Thing.find(1)
    unless t.score.nil?
      t.score > 0
    end
  end
  def self.or_form
    t = Thing.find(1)
    t.score.nil? || t.score > 0
  end
  def self.two
    t = Thing.find(1)
    t.score && t.other && t.score < t.other
  end"#,
    ));
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_unguarded_or_wrongly_guarded_read_is_still_reported() {
    let found = binops(&thing_with(
        r#"  def self.a
    t = Thing.find(1)
    t.score >= 0.5
  end
  def self.b
    t = Thing.find(1)
    t.score || t.score > 0
  end
  def self.c
    t = Thing.find(1)
    t.other && t.score > 0
  end
  def self.d
    t = Thing.find(1)
    u = Thing.find(2)
    t.score && u.score > 0
  end"#,
    ));
    assert_eq!(found.len(), 4, "{found:?}");
}

#[test]
fn a_user_defined_reader_of_the_same_name_does_not_narrow() {
    let found = binops(&thing_with(
        r#"  def score
    rand < 0.5 ? nil : 1.0
  end
  def self.keep
    Thing.all.select { |t| t.score && t.score >= 0.5 }
  end"#,
    ));
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn rebinding_or_writing_through_the_local_after_the_guard_does_not_narrow() {
    let found = binops(&thing_with(
        r#"  def self.rebind
    t = Thing.find(1)
    u = Thing.find(2)
    if t.score
      t = u
      t.score >= 0.5
    end
  end
  def self.setter
    t = Thing.find(1)
    if t.score
      t.score = nil
      t.score >= 0.5
    end
  end
  def self.reload
    t = Thing.find(1)
    t.score && (t.reload; t.score >= 0.5)
  end
  def self.shadow
    t = Thing.find(1)
    t.score && Thing.all.map { |t| t.score >= 0.5 }
  end"#,
    ));
    assert_eq!(found.len(), 4, "{found:?}");
}

const KEEP: &str = "  def self.keep\n    Thing.all.select { |t| t.score && t.score >= 0.5 }\n  end";

#[test]
fn a_reader_redefined_by_an_included_concern_does_not_narrow() {
    // A plain module `def`, an `included do` def, a module reached through another
    // module, and `alias_method` in the model all replace the column reader.
    let concerns = [
        ("app/models/concerns/scored.rb", "module Scored\n  def score\n    rand < 0.5 ? nil : 1.0\n  end\nend\n"),
        (
            "app/models/concerns/inner_scored.rb",
            "module InnerScored\n  def score\n    rand < 0.5 ? nil : 1.0\n  end\nend\n",
        ),
        ("app/models/concerns/outer_scored.rb", "module OuterScored\n  include InnerScored\n  def noop\n    1\n  end\nend\n"),
        (
            "app/models/concerns/hooked.rb",
            "module Hooked\n  extend ActiveSupport::Concern\n  included do\n    def score\n      rand < 0.5 ? nil : 1.0\n    end\n  end\nend\n",
        ),
    ];
    for (inc, file) in [("Scored", 0), ("OuterScored", 2), ("Hooked", 3)] {
        let found = binops_with(
            &format!("class Thing < ApplicationRecord\n  include {inc}\n{KEEP}\nend\n"),
            &concerns,
        );
        assert_eq!(found.len(), 1, "include {inc} ({}): {found:?}", concerns[file].0);
    }
    // Same nullable shape as the concern `def score` cases above: a
    // non-nil `1.0` alias would type as Float and never raise
    // IncompatibleBinop, which would not exercise the override guard.
    let found = binops(&format!(
        "class Thing < ApplicationRecord\n  def other_score\n    rand < 0.5 ? nil : 1.0\n  end\n  alias_method :score, :other_score\n{KEEP}\nend\n"
    ));
    assert_eq!(found.len(), 1, "alias_method: {found:?}");
    // The control: an included concern that does not touch the reader still narrows.
    let found = binops_with(
        &format!("class Thing < ApplicationRecord\n  include Inert\n{KEEP}\nend\n"),
        &[("app/models/concerns/inert.rb", "module Inert\n  def unrelated\n    1\n  end\nend\n")],
    );
    assert!(found.is_empty(), "{found:?}");
}
