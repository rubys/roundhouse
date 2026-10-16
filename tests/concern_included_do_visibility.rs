//! `private` / `protected` inside a concern's `included do … end` block
//! are applied, not rejected as dynamic.
//!
//! `included do … end` runs under `class_eval` on the includer, so
//! `private` there is as static as a `private` written directly in a
//! class body — but `reject_dynamic_declarations` flagged ANY marker
//! call found below the class body (an `included do` block is a call's
//! block), which failed the whole file's ingest: `conditional or
//! dynamic visibility declarations are not modeled`, cascading into
//! `include not supported (all targets): Gadget includes unresolved
//! GadgetTools`. The module was dropped whole, not because of anything
//! `polish`/`shine` do, but because `private` appears below the class
//! body at all.
//!
//! Splicing `included do`'s defs onto the includer's own emitted body is
//! a separate, pre-existing gap this fix does not touch — confirmed
//! below (`gadget_rb` contains neither `polish` nor `shine` even once
//! ingest succeeds, with or without the marker) — so the regression
//! proof here is ingest succeeding where it used to fail loud, which is
//! the whole of what `src/ingest/visibility.rs` controls.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files.iter().map(|(p, c)| (PathBuf::from(*p), c.as_bytes().to_vec())).collect()
}

const SCHEMA: &str =
    "ActiveRecord::Schema.define do\n  create_table \"gadgets\", force: :cascade do |t|\n  end\nend\n";

fn gadget_tools(included_body: &str) -> String {
    format!(
        "module GadgetTools\n  extend ActiveSupport::Concern\n\n  included do\n{included_body}\n  end\nend\n"
    )
}

fn gadget_app(
    included_body: &str,
) -> Result<roundhouse::App, roundhouse::ingest::IngestError> {
    ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        (
            "app/models/gadget.rb",
            "class Gadget < ApplicationRecord\n  include GadgetTools\nend\n",
        ),
        ("app/models/concerns/gadget_tools.rb", &gadget_tools(included_body)),
    ]))
}

/// The bare-marker form: `private` with no arguments switches the
/// default for the defs that follow it, same as in an ordinary class
/// body.
#[test]
fn included_do_bare_private_is_applied_not_rejected() {
    gadget_app("    def polish = \"#{shine}!\"\n\n    private\n\n    def shine = \"shiny\"\n")
        .expect("GadgetTools ingests: `private` inside `included do` is applied, not rejected");
}

/// The named-argument form: `private :x` after the fact.
#[test]
fn included_do_named_private_is_applied_not_rejected() {
    gadget_app("    def polish = \"#{shine}!\"\n    def shine = \"shiny\"\n    private :shine\n")
        .expect(
            "GadgetTools ingests: `private :shine` inside `included do` is applied, not rejected",
        );
}

/// `protected`, the third marker the task calls out alongside `private`.
#[test]
fn included_do_protected_is_applied_not_rejected() {
    gadget_app("    def polish = \"#{shine}!\"\n\n    protected\n\n    def shine = \"shiny\"\n")
        .expect("GadgetTools ingests: `protected` inside `included do` is applied, not rejected");
}

/// Thor's command-annotation DSL (`desc`, `long_desc`, `option`, …)
/// describes the next command and defines no methods — a bare marker
/// is still applied alongside one, the same as alongside a plain `def`.
/// This is the Mastodon shape (`lib/mastodon/cli/federation.rb`:
/// `included do; desc …; long_desc …; def self_destruct; …; private;
/// def domain_match_confirmed?; …; end; end`) `marker_mixed_with_unmodeled_call`
/// otherwise rejected: not because `desc`/`long_desc` generate
/// anything, just because they weren't on its allowlist. Like the three
/// `included_do_*_is_applied_not_rejected` tests above, and for the same
/// reason (`included_do_defs_are_not_yet_spliced_into_the_model_body`'s
/// pre-existing, unrelated gap — confirmed by probing `GadgetTools`'s own
/// `LibraryClass.methods`, which stays empty whether or not a def's
/// `included do` block ingests clean), the regression proof available
/// today is ingest succeeding where it used to fail loud, not a
/// per-method visibility read-back.
#[test]
fn included_do_marker_coexists_with_bare_thor_annotations() {
    gadget_app(
        "    desc \"polish\", \"Polish it\"\n    long_desc \"explains polishing in depth\"\n    \
         option :force, type: :boolean\n    def polish = shine\n\n    private\n\n    def shine = \"shiny\"\n",
    )
    .expect("GadgetTools ingests: Thor annotations beside `private` are not an unmodeled call");
}

/// Other dynamic or conditional visibility inside `included do … end`
/// still fails loud — this fix only recognizes the static `private` /
/// `protected` forms, not a marker guarded by a runtime condition.
#[test]
fn included_do_dynamic_visibility_stays_rejected() {
    let err = gadget_app(
        "    def polish = \"#{shine}!\"\n\n    if Rails.env.test?\n      private\n    end\n\n    def shine = \"shiny\"\n",
    )
    .expect_err("conditional visibility inside included do stays rejected");
    assert!(
        format!("{err:?}").contains("not modeled"),
        "expected a not-modeled visibility error, got: {err:?}"
    );
}

/// A marker coexisting with some OTHER call this file's visibility
/// tracking does not itself attribute a method to (`delegate`, here)
/// stays rejected too: accepting it silently would risk shipping the
/// delegated method PUBLIC once something downstream splices it onto
/// the includer, since nothing here verifies `delegate`'s visibility.
/// Same shape `tests/attachment_model_lowering.rs`'s
/// `a_private_delegate_in_an_included_block_is_rejected_at_ingest`
/// already covers on a real concern; this is the generic-fixture
/// companion.
#[test]
fn included_do_marker_mixed_with_an_untracked_call_stays_rejected() {
    let err = gadget_app("    private\n\n    delegate :shine, to: :tool\n")
        .expect_err("a marker mixed with an untracked declaration stays rejected");
    assert!(
        format!("{err:?}").contains("not modeled"),
        "expected a not-modeled visibility error, got: {err:?}"
    );
}

/// An explicit `self.` receiver is the includer, same as no receiver at
/// all — `self.delegate` must be caught exactly like a bare `delegate`
/// call. Fails on b6379ca9 (the receiver check skipped it, so this
/// ingested and would have shipped `shine` public once something
/// downstream splices the delegate onto the includer).
#[test]
fn included_do_marker_mixed_with_a_self_qualified_untracked_call_stays_rejected() {
    let err = gadget_app("    private\n\n    self.delegate :shine, to: :tool\n")
        .expect_err("a marker mixed with a self-qualified untracked declaration stays rejected");
    assert!(
        format!("{err:?}").contains("not modeled"),
        "expected a not-modeled visibility error, got: {err:?}"
    );
}

/// An includer guard (`if`/`unless`) around the untracked call is no
/// safer than writing it directly in the block. Fails on b6379ca9 (the
/// walker only looked at direct, receiverless top-level statements, so
/// a `delegate` nested inside an `if` passed through unseen).
#[test]
fn included_do_marker_mixed_with_an_untracked_call_inside_a_conditional_stays_rejected() {
    let err = gadget_app(
        "    private\n\n    if true\n      delegate :shine, to: :tool\n    end\n",
    )
    .expect_err("a marker mixed with a conditionally-guarded untracked declaration stays rejected");
    assert!(
        format!("{err:?}").contains("not modeled"),
        "expected a not-modeled visibility error, got: {err:?}"
    );
}

/// Splicing `included do`'s plain defs onto the includer's own emitted
/// body is a separate, pre-existing gap (reproduces identically with no
/// marker at all, e.g. the `Hooked` module in
/// `tests/binop_member_guard.rs`) — not something this fix changes.
/// Documented here so a future fix to THAT gap does not mistake this
/// test's silence for a claim it already works.
#[test]
fn included_do_defs_are_not_yet_spliced_into_the_model_body() {
    use roundhouse::emit::ruby;

    let app = gadget_app("    def polish = \"#{shine}!\"\n\n    private\n\n    def shine = \"shiny\"\n")
        .expect("ingest");
    let files = ruby::emit_lowered_models(&app);
    let gadget_rb = files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("app/models/gadget.rb"))
        .map(|f| f.content.clone())
        .expect("gadget.rb");
    assert!(
        !gadget_rb.contains("def polish") && !gadget_rb.contains("def shine"),
        "if this starts failing, `included do` splicing has landed — update/remove this test \
         and add a true CRuby `Gadget.new.polish == \"shiny!\"` / private-`shine` proof instead:\n{gadget_rb}"
    );
}
