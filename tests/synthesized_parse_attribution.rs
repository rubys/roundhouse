//! Regression for a misattribution bug in the synthesizing lowering
//! passes (`ingest::delegate` and its siblings: `current_attributes`,
//! `channel_callbacks`, `allow_browser`, `rate_limit`).
//!
//! Each of those passes renders a macro declaration (`delegate`,
//! `on_subscribe`, …) as Ruby source and re-ingests it so the
//! generated bodies are ordinary IR. Prism is error-recovering, so a
//! bug in that rendering doesn't fail the re-ingest outright — it
//! silently hands back a malformed AST alongside parse diagnostics.
//! Before the fix, those diagnostics rode the SAME thread-local buffer
//! as the rest of the app's parse diagnostics, resolving their
//! `FileId` against a source registry that had already been `drain`ed
//! and restarted numbering from 1 for the synthesized re-ingest — the
//! same `FileId` `config/initializers/inflections.rb` (the first real
//! file the whole-app ingest registers) had been assigned under the
//! ORIGINAL numbering baked into `App::sources`. The result: a bug in
//! delegate-forwarder synthesis rendered as a phantom syntax error in
//! `inflections.rb`, a file that was never touched.
//!
//! This app has exactly one library class with a delegated setter
//! (`delegate :behavior, :behavior=, to: :deprecator`, the shape
//! `procore_os/deprecation.rb` actually declares) and an otherwise
//! unremarkable, valid `inflections.rb`. It must ingest with zero
//! parse diagnostics. A second scenario forces an actual synthesis
//! failure (a delegate name Ruby can't spell as a bare `def`) and
//! checks it lands on the survey ledger, attributed to the real
//! file — never as a parse diagnostic, and never against
//! `inflections.rb`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::{ingest_app_from_tree, prism};

const INFLECTIONS: &[u8] =
    b"ActiveSupport::Inflector.inflections(:en) do |inflect|\n  inflect.irregular \"leaf\", \"leaves\"\nend\n";

fn base_tree() -> HashMap<PathBuf, Vec<u8>> {
    [
        (PathBuf::from("config/initializers/inflections.rb"), INFLECTIONS.to_vec()),
        (
            PathBuf::from("db/schema.rb"),
            b"ActiveRecord::Schema.define do\nend\n".to_vec(),
        ),
        (
            PathBuf::from("config/routes.rb"),
            b"Rails.application.routes.draw do\nend\n".to_vec(),
        ),
    ]
    .into_iter()
    .collect()
}

/// A class shaped like `app/lib/procore_os/deprecation.rb`: a plain
/// library class (not `ApplicationRecord`) with a getter/setter pair
/// delegated to an `attr_accessor`-backed target.
const DEPRECATION: &[u8] = b"class Deprecation\n  attr_accessor :deprecator\n\n  delegate :behavior, :behavior=, to: :deprecator\nend\n";

#[test]
fn a_delegated_setter_produces_zero_parse_diagnostics() {
    let mut tree = base_tree();
    tree.insert(PathBuf::from("app/lib/deprecation.rb"), DEPRECATION.to_vec());

    // The same outer scope `roundhouse-check` installs around the
    // whole-app ingest (`src/cli.rs`) — this is what would have
    // collected the phantom `inflections.rb` errors before the fix.
    let (result, parse_diags) = prism::scope(|| ingest_app_from_tree(tree));
    let app = result.expect("ingest should succeed");

    assert!(
        parse_diags.is_empty(),
        "expected no parse diagnostics, got: {:?}",
        parse_diags
            .iter()
            .map(|d| d.render(&app.sources))
            .collect::<Vec<_>>()
    );

    let lc = app
        .library_classes
        .iter()
        .find(|c| c.name.0.as_str() == "Deprecation")
        .expect("Deprecation should be ingested as a library class");
    assert!(
        lc.methods.iter().any(|m| m.name.as_str() == "behavior"),
        "the getter should have been synthesized: {:?}",
        lc.methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
    );
    assert!(
        lc.methods.iter().any(|m| m.name.as_str() == "behavior="),
        "the setter should have been synthesized: {:?}",
        lc.methods.iter().map(|m| m.name.as_str()).collect::<Vec<_>>()
    );
}

/// `:"1abc"` is a legal Ruby symbol literal — quoted because a bare
/// `:1abc` isn't valid symbol syntax — but splicing its value bare
/// into `def <name>` IS a syntax error (a method name can't start
/// with a digit; Ruby would otherwise read it as an integer literal).
/// It's a genuine unsynthesizable shape (not contrived to force a
/// failure): a delegate whose declared name isn't a valid bare
/// method-name token, which `take_delegate_decls` has no reason to
/// reject up front — it IS a `Literal::Sym`, the same shape as
/// `:behavior`. (A quoted symbol with an embedded space, the first
/// shape tried here, turned out NOT to reproduce this: Ruby's
/// paren-optional `def` syntax happily reads `def weird name` as
/// method `weird` taking a parameter `name`, so it round-trips as
/// valid, if not what the app author meant.)
const UNSYNTHESIZABLE_NAME: &[u8] =
    b"class Deprecation\n  attr_accessor :deprecator\n\n  delegate :\"1abc\", to: :deprecator\nend\n";

#[test]
fn an_unsynthesizable_delegate_name_becomes_a_survey_gap_not_a_parse_diagnostic() {
    let mut tree = base_tree();
    tree.insert(PathBuf::from("app/lib/deprecation.rb"), UNSYNTHESIZABLE_NAME.to_vec());

    roundhouse::ingest::survey::activate();
    let (result, parse_diags) = prism::scope(|| ingest_app_from_tree(tree));
    let survey_errors = roundhouse::ingest::survey::drain();
    let app = result.expect("ingest should succeed even though the delegate could not be synthesized");

    // The whole point of isolating the re-ingest in its own
    // `prism::scope`: this pass's own synthesis bug must not surface
    // as one of the APP's parse diagnostics at all...
    assert!(
        parse_diags.is_empty(),
        "a synthesis failure must not leak into the app's own parse diagnostics: {:?}",
        parse_diags
            .iter()
            .map(|d| d.render(&app.sources))
            .collect::<Vec<_>>()
    );
    // ...it belongs on the survey ledger instead — roundhouse's gap,
    // not the app's (AGENTS.md invariant 6) — attributed to the real
    // declaring file (resolved from the `delegate` call's own span),
    // not the synthesizing pass's `"<delegate>"` label.
    assert!(
        survey_errors.iter().any(|e| matches!(
            e,
            roundhouse::ingest::IngestError::Unsupported { file, message }
                if message.contains("could not be synthesized")
                    && file == "app/lib/deprecation.rb"
        )),
        "expected a survey gap recording the synthesis failure against the real file, got: {survey_errors:?}"
    );
}

#[test]
fn a_delegated_setter_never_attributes_a_synthesis_bug_to_inflections_rb() {
    // Belt-and-suspenders on top of the zero-diagnostics assertions
    // above: even for the deliberately unsynthesizable shape, whose
    // re-ingest DOES produce Prism parse errors internally (just
    // isolated and converted, never surfaced), nothing renders against
    // `inflections.rb` — the file that absorbed this exact bug before
    // the fix.
    let mut tree = base_tree();
    tree.insert(PathBuf::from("app/lib/deprecation.rb"), UNSYNTHESIZABLE_NAME.to_vec());

    let (result, parse_diags) = prism::scope(|| ingest_app_from_tree(tree));
    let app = result.expect("ingest should succeed");

    for d in &parse_diags {
        let rendered = d.render(&app.sources);
        assert!(
            !rendered.contains("inflections.rb"),
            "a parse diagnostic must never attribute to inflections.rb: {rendered}"
        );
    }
}
