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

use roundhouse::diagnostic::{Diagnostic, Severity};
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::BuildTarget;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// Every diagnostic the post-analyze pipeline produced for the app
/// below, unfiltered — the residue entries are the ones whose message
/// says "relation chain stays dynamic".
fn diagnostics() -> Vec<Diagnostic> {
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

  def self.grouped_count
    Story.group(:score).count
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
    roundhouse::session::analyze_and_lower(&mut app)
}

/// Every `dynamic_relation` message the ledger produced, one per entry.
fn ledgered() -> Vec<String> {
    diagnostics()
        .iter()
        .map(Diagnostic::to_string)
        .filter(|d| d.contains("relation chain stays dynamic"))
        .collect()
}

/// The residue entries themselves, after the emit-bound driver has
/// decided their severity for `target`.
fn residue_for(target: BuildTarget) -> Vec<Diagnostic> {
    let mut diags = diagnostics();
    roundhouse::lower::relation_residue::elevate_dynamic_relation_for_target(
        &mut diags, target,
    );
    diags
        .into_iter()
        .filter(|d| d.message.contains("relation chain stays dynamic"))
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

#[test]
fn a_grouped_count_is_not_ledgered() {
    // `group(:col).count` folds — the builder recognizes the pair (see
    // `tests/arel_group_count.rs`). The chain reads unfoldable
    // link-by-link, though: the terminal is Hash-typed, so it is not a
    // relation head, and the `group(:col)` beneath it is Relation-typed
    // and declines to fold ON ITS OWN by design. A ledger that asked
    // only about Relation-typed links counted it as residue — and on a
    // relation-less target that failed the emit of a chain that folds.
    let entries = ledgered();
    assert!(
        !entries.iter().any(|d| d.contains("`group`")),
        "a chain claimed WHOLE by the builder must not be ledgered \
         link by link, got: {entries:?}",
    );
}

#[test]
fn a_ruby_family_target_keeps_the_residue_a_warning() {
    // `ruby`, `jruby` and `spinel` emit
    // `runtime/ruby/active_record/relation.rb`, so an unfolded chain
    // really does execute. A missed specialization, priced by the
    // ledger — not a defect.
    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        let residue = residue_for(target);
        assert!(!residue.is_empty(), "the app under test has residue");
        assert!(
            residue.iter().all(|d| d.severity == Severity::Warning),
            "{} has a runtime Relation: {:?}",
            target.as_str(),
            residue.iter().map(Diagnostic::to_string).collect::<Vec<_>>(),
        );
    }
}

#[test]
fn a_relation_less_target_fails_on_the_residue_it_predicts() {
    // Issue #76. The residue's own text says "unsupported at
    // strict-target emit", and that was a WARNING — suppressed by
    // default, so `roundhouse --target rust` wrote a call into a type
    // nothing in the emitted tree defines and exited 0. Nothing under
    // `runtime/{rust,go,crystal,python,typescript}` defines a Relation
    // or any of its methods.
    for target in [
        BuildTarget::Rust,
        BuildTarget::Go,
        BuildTarget::Crystal,
        BuildTarget::Python,
        BuildTarget::Typescript,
    ] {
        let residue = residue_for(target);
        assert!(!residue.is_empty(), "the app under test has residue");
        assert!(
            residue.iter().all(|d| d.severity == Severity::Error),
            "{} has no runtime Relation to execute the chain on: {:?}",
            target.as_str(),
            residue.iter().map(Diagnostic::to_string).collect::<Vec<_>>(),
        );
        assert!(
            residue.iter().all(|d| d.message.contains(target.as_str())),
            "the elevated message names the target it is elevated for",
        );
    }
}

#[test]
fn only_the_dynamic_relation_residue_is_elevated() {
    // The elevation is keyed on the pass and construct, not on the
    // `LowerResidue` kind: every other residue channel keeps whatever
    // severity its own pass chose.
    let mut diags = diagnostics();
    let before: Vec<String> = diags
        .iter()
        .filter(|d| !d.message.contains("relation chain stays dynamic"))
        .map(Diagnostic::to_string)
        .collect();
    roundhouse::lower::relation_residue::elevate_dynamic_relation_for_target(
        &mut diags,
        BuildTarget::Rust,
    );
    let after: Vec<String> = diags
        .iter()
        .filter(|d| !d.message.contains("relation chain stays dynamic"))
        .map(Diagnostic::to_string)
        .collect();
    assert_eq!(before, after, "no other diagnostic is touched");
}
