//! The `dynamic_relation` ledger counts chains that stay DYNAMIC —
//! not every chain that happens to be Relation-typed when the ledger
//! runs (`lower::relation_residue`).
//!
//! The ledger runs inside `apply_post_analyze_lowerings`; the Arel
//! materializer runs later still, inside `controller_to_library` /
//! `model_to_library`. So a `Model.where(...)` chain is Relation-typed
//! at ledger time and direct SQL by emit. Until class-side chain starts
//! converged onto `Ty::Relation` (docs/relation-convergence-plan.md C1)
//! the gap was invisible — the only Relation-typed heads were
//! scope-rooted, which `try_build_arel` never lifts anyway. After
//! convergence it counted folded chains as residue, including one on
//! real-blog, which the playground's "baseline is clean" smoke check
//! caught.
//!
//! These pin both directions: a foldable head is NOT ledgered, an
//! unfoldable one IS, and the count is per chain HEAD rather than per
//! link.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::diagnostic::Diagnostic;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// Every `dynamic_relation` message the ledger produced, one per entry.
fn ledgered() -> Vec<String> {
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            r#"ActiveRecord::Schema.define do
  create_table "stories", force: :cascade do |t|
    t.string "title", null: false
    t.integer "score", null: false
  end
end
"#,
        ),
        (
            "app/models/story.rb",
            r#"class Story < ApplicationRecord
  scope :recent, -> { limit(10) }

  def self.folds
    Story.where(score: 1)
  end

  def self.stays_dynamic
    Story.order("score asc")
  end
end
"#,
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/\", to: \"home#index\"\nend\n",
        ),
    ]))
    .expect("ingest");
    let diags = roundhouse::session::analyze_and_lower(&mut app);
    diags
        .iter()
        .map(Diagnostic::to_string)
        .filter(|d| d.contains("relation chain stays dynamic"))
        .collect()
}

#[test]
fn a_foldable_chain_is_not_ledgered() {
    // `Story.where(score: 1)` is exactly the shape `try_build_arel`
    // lifts to a SELECT. It is Relation-typed when the ledger runs and
    // direct SQL by emit, so counting it would be counting a chain that
    // specializes seconds later.
    let entries = ledgered();
    assert!(
        !entries.iter().any(|d| d.contains("`where`")),
        "a chain the Arel builder folds must not be ledgered, got: {entries:?}",
    );
}

#[test]
fn an_unfoldable_chain_is_ledgered() {
    // A string `order` argument has no ColumnSpec, so the builder
    // declines and the chain really does execute on the runtime
    // Relation. That is the residue the ledger exists to price.
    let entries = ledgered();
    assert!(
        entries.iter().any(|d| d.contains("`order`")),
        "a chain that stays dynamic must be ledgered, got: {entries:?}",
    );
}

#[test]
fn an_implicit_self_scope_body_is_ledgered() {
    // `scope :recent, -> { limit(10) }` has no receiver to root an
    // Arel base at, and lowers to a call on the runtime Relation
    // (`__rel.limit(10)`) — dynamic, and correctly counted. Guards
    // against "fold-aware" being implemented as "skip anything whose
    // method name the builder recognizes."
    let entries = ledgered();
    assert!(
        entries.iter().any(|d| d.contains("`limit`")),
        "an implicit-self scope body stays dynamic and must be ledgered, got: {entries:?}",
    );
}
