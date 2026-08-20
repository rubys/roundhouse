//! A form-wrapper helper whose tail is an OPTIONS HASH.
//!
//! `form_wrapper_helpers` splices a helper that is nothing but a
//! `form_with` with the block forwarded through, so the form-builder
//! macro-inline sees both halves at once. It declined campfire's
//! `Users::ProfilesHelper#profile_form_with(model, **params, &)` three
//! ways, and the helper emitted a bare `form_with` no module defines:
//!
//! * `**options` ingests as a trailing POSITIONAL defaulting to `{}`,
//!   so `profile_form_with @user` supplies one argument to a
//!   two-parameter wrapper and the arity check rejected it. The same
//!   template also writes `profile_form_with @user, class: "…"`, so
//!   both spellings have to work.
//! * A literal options hash is not a `Var`/`Ivar`/`Lit`, so the
//!   pure-read check rejected the call that DID have full arity. A hash
//!   literal of pure reads is built fresh at the call site —
//!   substituting it re-allocates rather than re-observes, which is the
//!   property that predicate is about.
//! * And once spliced, `lower::kwsplat` has already desugared the
//!   wrapper's body to `{…}.merge(params)`, where `form_with` wants ONE
//!   literal hash to walk. Given the chain it DECLINES — and declining
//!   after the splice is worse than declining before it: the call is
//!   gone and nothing replaces it, so the form silently vanishes from
//!   the page instead of failing loudly. Measured: three
//!   `profile_form_with` calls became three `io << ""`.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::app::App;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

/// The FULL pipeline, not just `lower_view_to_library_class`: the
/// `**params` → `.merge` desugar is a post-analyze pass, so a harness
/// that skips those never sees the shape this fix is about.
fn app_with(call_tail: &str) -> App {
    let template = format!(
        "<%= thing_form_with @thing{call_tail} do |form| %>\n  <p>hi</p>\n<% end %>\n"
    );
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :things\nend\n",
        ),
        ("app/models/thing.rb", "class Thing < ApplicationRecord\nend\n"),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  def show\n    @thing = Thing.first\n  end\nend\n",
        ),
        (
            "app/helpers/things_helper.rb",
            r#"module ThingsHelper
  def thing_form_with(model, **params, &)
    form_with model: model, url: thing_path(model), method: :patch, **params, &
  end
end
"#,
        ),
        ("app/views/things/show.html.erb", Box::leak(template.into_boxed_str())),
    ]))
    .expect("ingest wrapper app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn show_view(call_tail: &str) -> String {
    let app = app_with(call_tail);
    let files = roundhouse::emit::ruby::emit_lowered_views(&app);
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with("things/show.rb"))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no show view; got {:?}",
                files.iter().map(|f| f.path.display().to_string()).collect::<Vec<_>>()
            )
        })
}

/// Bare call — the options parameter binds its own `{}` default.
#[test]
fn a_call_with_no_options_binds_the_default() {
    let src = show_view("");
    assert!(src.contains("<form"), "the wrapper must splice to a real form:\n{src}");
    assert!(
        !src.contains("thing_form_with"),
        "no call to the un-spliced wrapper may survive:\n{src}"
    );
}

/// …and with a literal options hash, whose keys reach the form and
/// whose collisions WIN, which is Ruby's `merge` and the point of a
/// wrapper.
#[test]
fn a_call_with_an_options_hash_merges_and_overrides() {
    let src = show_view(", class: \"txt-medium\", method: :post");
    assert!(src.contains("<form"), "{src}");
    assert!(!src.contains("thing_form_with"), "{src}");
    assert!(src.contains("txt-medium"), "the caller's options reach the form:\n{src}");
    assert!(
        src.contains("form_method = :post"),
        "the caller's `method:` overrides the helper's `:patch`:\n{src}"
    );
}

/// The failure this fix is really about: never an EMPTY append where a
/// form belongs.
#[test]
fn the_form_is_never_silently_dropped() {
    for tail in ["", ", class: \"x\""] {
        let src = show_view(tail);
        assert!(
            !src.contains("io << \"\"\n      io\n"),
            "the whole tag vanished for `{tail}`:\n{src}"
        );
    }
}
