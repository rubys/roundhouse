//! A signature that names `ActiveRecord::Relation` without a model is a
//! relation of something, not a class with no methods.
//!
//! ```ruby
//! #: () -> ActiveRecord::Relation
//! def phases
//!   promotion_phases.order(:order)
//! end
//!
//! def find_phase(name) = phases.find_by(name: name)
//! ```
//!
//! A large app writes the bare name in more than two thousand signatures (Sorbet's
//! `PrivateRelation` names have no RBS spelling). The registry has no
//! `ActiveRecord::Relation` class, so every query on the declared value
//! (`where`, `exists?`, `find_by`, `each`) was `no known method`. The model
//! the relation ranges over is not written, so the value is gradual, the same
//! as `untyped`; a relation the body or a scope infers keeps its model.

use roundhouse::analyze::{diagnose, Analyzer, DiagnosticKind};

fn dispatch_errors(model: &str) -> Vec<String> {
    let tree: std::collections::HashMap<std::path::PathBuf, Vec<u8>> = [
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/phase.rb", "class Phase < ApplicationRecord\nend\n"),
        ("app/models/promotion.rb", model),
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table \"phases\", force: :cascade do |t|\n    t.string \"name\"\n  end\n  create_table \"promotions\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
    ]
    .iter()
    .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree");
    Analyzer::new(&app).analyze(&mut app);
    diagnose(&app)
        .into_iter()
        .filter(|d| {
            matches!(
                d.kind,
                DiagnosticKind::SendDispatchFailed { .. } | DiagnosticKind::IncompatibleBinop { .. }
            )
        })
        .map(|d| format!("{:?}", d.kind))
        .collect()
}

#[test]
fn a_bare_relation_signature_answers_the_query_surface() {
    let found = dispatch_errors(
        r#"class Promotion < ApplicationRecord
  #: () -> ActiveRecord::Relation
  def phases
    Phase.where(name: "a")
  end

  sig { returns(ActiveRecord::Relation) }
  def recent_phases
    Phase.order(:name)
  end

  #: () -> ActiveRecord::Relation?
  def maybe_phases
    nil
  end

  #: (ActiveRecord::Relation scope) -> Integer
  def count_of(scope)
    scope.where(name: "b").count
  end

  def probe(name)
    phases.find_by(name: name)
    phases.exists?
    recent_phases.where(name: name).each { |phase| phase }
    maybe_phases&.update_all(name: name)
    count_of(phases)
  end
end
"#,
    );
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_inferred_relation_keeps_its_model() {
    let found = dispatch_errors(
        r#"class Promotion < ApplicationRecord
  def phases
    Phase.where(name: "a")
  end

  def probe
    phases.first.imaginary_method
  end
end
"#,
    );
    assert_eq!(found.len(), 1, "{found:?}");
}
