//! An app's own `sig` blocks, read as signature seeds.
//!
//! The annotations sit where inference runs out: a call into a gem the
//! catalog does not model answers an untyped value, and the `sig` on
//! the method that consumes it names the type that was lost. Reading
//! them recovers the boundary without the gem ever being modeled.
//!
//! The policy these tests state, because the choice is not obvious:
//! **the annotation seeds the table, and inference fills what it does
//! not cover.** A `sig` this reader cannot represent is dropped whole —
//! silently, the way `spinel_rbs_extract` drops RBS it cannot express —
//! and the method is inferred as before. A `sig/**/*.rbs` sidecar wins
//! over a `sig` for the same method: it is written for this analyzer,
//! the `sig` is written for Sorbet.
//!
//! Measured limits, so they are not a surprise: the seeds reach the
//! analyzer's dispatch table — `check`, the LSP and the MCP tools see
//! them — but not the RBS sidecar the Spinel lane is built from, which
//! takes a method's return type from the lowered function rather than
//! from `rbs_signatures`. And a `sig` on a `def self.x` lands in the
//! same undifferentiated table the `sig/**/*.rbs` reader uses, where
//! the singleton emit path does not read it.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ident::{ClassId, Symbol};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::ty::Ty;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "reports", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

/// A use case whose base class comes from a gem nothing models, so the
/// result of `.call` — and `.data` on it — is untyped.
const INTERACTOR: &str = r#"module Interactors
  class BuildReport < Vendor::Base
    def self.call
      new.run
    end
  end
end
"#;

fn tree(controller: &str) -> HashMap<PathBuf, Vec<u8>> {
    [
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n"),
        ("app/models/report.rb", "class Report < ApplicationRecord\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/controllers/reports_controller.rb", controller),
        ("app/services/build_report.rb", INTERACTOR),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/reports\", to: \"reports#show\"\nend\n",
        ),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect()
}

fn app_for(controller: &str) -> roundhouse::App {
    ingest_app_from_tree(tree(controller)).expect("ingest")
}

fn signature(app: &roundhouse::App, class: &str, method: &str) -> Option<Ty> {
    app.rbs_signatures
        .get(&ClassId(Symbol::new(class)))
        .and_then(|methods| methods.get(&Symbol::new(method)))
        .cloned()
}

fn returns_of(ty: &Ty) -> Option<Ty> {
    match ty {
        Ty::Fn { ret, .. } => Some((**ret).clone()),
        _ => None,
    }
}

const WITH_SIG: &str = r#"class ReportsController < ApplicationController
  sig { params(name: String).returns(String) }
  def slug_for(name)
    name.downcase
  end

  def show
    data = Interactors::BuildReport.call.data
    @slug = slug_for(data)
  end
end
"#;

#[test]
fn a_sig_declares_the_method_the_analyzer_could_not_infer() {
    let app = app_for(WITH_SIG);
    let ty = signature(&app, "ReportsController", "slug_for").expect("the sig is read");
    assert_eq!(returns_of(&ty), Some(Ty::Str), "the declared return type is what lands");
    let Ty::Fn { params, .. } = &ty else { panic!("expected a function type, got {ty:?}") };
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, Symbol::new("name"));
    assert_eq!(params[0].ty, Ty::Str);
}

#[test]
fn a_signature_outside_the_grammar_is_dropped_whole() {
    // `T.type_parameter` is Sorbet's generics, which this reader does
    // not model. The method keeps whatever inference makes of it; what
    // must not happen is a half-read signature.
    let controller = r#"class ReportsController < ApplicationController
  sig { type_parameters(:U).params(item: T.type_parameter(:U)).returns(T.type_parameter(:U)) }
  def echo(item)
    item
  end

  def show
    @slug = echo("x")
  end
end
"#;
    let app = app_for(controller);
    assert_eq!(
        signature(&app, "ReportsController", "echo"),
        None,
        "an unrepresentable signature is not half-read"
    );
}

#[test]
fn the_grammar_covers_the_forms_an_app_actually_writes() {
    let controller = r#"class ReportsController < ApplicationController
  sig { void }
  def a; end

  sig { returns(T.nilable(String)) }
  def b; end

  sig { params(xs: T::Array[String], flag: T::Boolean).returns(T::Hash[Symbol, Integer]) }
  def c(xs, flag); end

  sig { override.params(name: String).returns(Symbol) }
  def d(name); end

  sig { params(value: T.untyped).returns(T.any(String, Integer)) }
  def e(value); end

  def show; end
end
"#;
    let app = app_for(controller);
    let ret = |m: &str| returns_of(&signature(&app, "ReportsController", m).expect(m));
    assert_eq!(ret("a"), Some(Ty::Nil), "void is Nil");
    assert_eq!(
        ret("b"),
        Some(Ty::Union { variants: vec![Ty::Str, Ty::Nil] }),
        "T.nilable is a union with Nil"
    );
    assert_eq!(
        ret("c"),
        Some(Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Int) })
    );
    assert_eq!(ret("d"), Some(Ty::Sym), "modifier forms carry no type of their own");
    assert_eq!(ret("e"), Some(Ty::Union { variants: vec![Ty::Str, Ty::Int] }));
    let Some(Ty::Fn { params, .. }) = signature(&app, "ReportsController", "c") else {
        panic!("c has a signature")
    };
    assert_eq!(params[0].ty, Ty::Array { elem: Box::new(Ty::Str) });
    assert_eq!(params[1].ty, Ty::Bool, "T::Boolean");
}

#[test]
fn the_annotation_wins_a_disagreement_with_the_body() {
    // The policy, stated so it is a decision rather than an accident:
    // a `sig` that disagrees with what the body returns is honoured,
    // and the disagreement is not reported today.
    //
    // It follows from the table rather than from anything this reader
    // does. `insert_inferred_return` (`src/analyze/mod.rs`) never
    // overwrites an existing `Ty::Fn`, which is the rule the
    // `sig/**/*.rbs` sidecar has always relied on — a seed is a seed
    // wherever it was written. That function is also where a
    // disagreement diagnostic would go: it is the one point at which
    // the declared type and the inferred one are both in hand.
    let controller = r#"class ReportsController < ApplicationController
  sig { returns(String) }
  def label
    42
  end

  def show
    @slug = label.upcase
  end
end
"#;
    let mut app = app_for(controller);
    assert_eq!(
        returns_of(&signature(&app, "ReportsController", "label").expect("the sig is read")),
        Some(Ty::Str)
    );
    let residue = roundhouse::session::analyze_and_lower(&mut app);
    let diagnostics: Vec<String> = residue
        .iter()
        .chain(roundhouse::analyze::diagnose(&app).iter())
        .map(roundhouse::diagnostic::Diagnostic::to_string)
        .collect();
    assert!(
        diagnostics.is_empty(),
        "`upcase` dispatches against the declared String, and the contradiction \
         with the Integer body is silent today; diagnostics = {diagnostics:?}"
    );
}
