//! An app helper handed `raw(x)` retargets to a `_raw` clone in which
//! that argument's escaping comes off.
//!
//! Rails carries safety in the value (SafeBuffer), so a safe label
//! rides a plain parameter into a helper and out to `link_to`, which
//! declines to escape it. Nothing survives that boundary on a
//! transpiled tree. lobsters' layout hits it once —
//! `link_to_different_page raw("…<span class='karma'>…"), settings_path`
//! — and the escaped result showed as a +24B diff against Rails on
//! THIRTEEN of the 49 benchmark routes (2026-07-26). Propagating the
//! exemption statically took the sweep from 5 byte-identical to 18.

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn emit(view_body: &str, helper_body: &str) -> String {
    let files: Vec<(&str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\nend\n".to_string(),
        ),
        ("app/helpers/application_helper.rb",
         format!("module ApplicationHelper\n{helper_body}end\n")),
        ("app/controllers/pages_controller.rb",
         "class PagesController < ApplicationController\n  def show\n  end\nend\n".to_string()),
        ("app/views/pages/show.html.erb", view_body.to_string()),
        ("config/routes.rb",
         "Rails.application.routes.draw do\n  get \"pages/show\" => \"pages#show\"\nend\n".to_string()),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.into_bytes()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let mut out = String::new();
    for f in ruby::emit_lowered_views(&app) {
        out.push_str(&f.content);
    }
    for f in ruby::emit_lowered_models(&app) {
        out.push_str(&f.content);
    }
    // App helpers are emitted as library classes, not models.
    for f in ruby::emit_library(&app) {
        out.push_str(&f.content);
    }
    out
}

const LINKER: &str = "  def fancy_link(text, path)\n    link_to text, path\n  end\n";

#[test]
fn a_raw_argument_retargets_the_call_and_synthesizes_the_clone() {
    let out = emit("<%= fancy_link raw(\"<b>hi</b>\"), \"/x\" %>\n", LINKER);
    assert!(
        out.contains("ApplicationHelper.fancy_link_raw(\"<b>hi</b>\""),
        "the call retargets and the raw() wrapper is consumed:\n{out}",
    );
    assert!(
        out.contains("def self.fancy_link_raw(text, path)"),
        "the clone is synthesized:\n{out}",
    );
    assert!(
        out.contains("link_to_raw(text"),
        "and inside it, link_to on the safe param becomes link_to_raw:\n{out}",
    );
}

#[test]
fn the_original_helper_survives_unchanged() {
    // Other call sites still escape — monomorphizing must not mutate
    // the general version.
    let out = emit(
        "<%= fancy_link raw(\"<b>hi</b>\"), \"/x\" %>\n<%= fancy_link \"plain\", \"/y\" %>\n",
        LINKER,
    );
    assert!(
        out.contains("def self.fancy_link(text, path)"),
        "the original is still emitted:\n{out}",
    );
    assert!(
        out.contains("ApplicationHelper.fancy_link(\"plain\""),
        "an unmarked argument keeps the escaping version:\n{out}",
    );
}

#[test]
fn no_raw_site_means_no_clone() {
    let out = emit("<%= fancy_link \"plain\", \"/y\" %>\n", LINKER);
    assert!(
        !out.contains("fancy_link_raw"),
        "the pass is a no-op without a raw() call site:\n{out}",
    );
}
