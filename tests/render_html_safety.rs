//! `render html:` escapes its body unless the body says it is safe.
//!
//! Rails escapes an `html:` body and honors a SafeBuffer mark. We used
//! to emit `html_escape(<body>)` unconditionally and lean on the CRuby
//! overlay's SafeString to make `html_escape(x.html_safe)` a runtime
//! pass-through — a CRuby-only fact. Under AOT it was two bugs at once:
//! `String#html_safe` raised (every `/u` visit on the lobsters spinel
//! lane, 15 of 114, 2026-07-26), and had it not raised, the body would
//! have been escaped where Rails leaves it alone.
//!
//! A mark written AT the render answers the escape question there, so
//! the pair cancels. lobsters' about_controller writes both shapes in
//! one file, which is the discrimination this pins.

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn controller_src(action_body: &str) -> String {
    let files: Vec<(&str, String)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\nend\n".to_string(),
        ),
        (
            "app/controllers/pages_controller.rb",
            format!("class PagesController < ApplicationController\n  def show\n{action_body}  end\nend\n"),
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"pages/show\" => \"pages#show\"\nend\n"
                .to_string(),
        ),
        ("app/views/pages/show.html.erb", "<p>show</p>\n".to_string()),
        (
            "app/views/layouts/application.html.erb",
            "<html><body><%= yield %></body></html>\n".to_string(),
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.into_bytes()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    ruby::emit_lowered_controllers(&app)
        .into_iter()
        .filter(|f| f.path.extension().is_some_and(|e| e == "rb"))
        .map(|f| f.content)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn an_unmarked_body_is_escaped() {
    let out = controller_src("    render html: \"<h1>A mystery.\"\n");
    assert!(
        out.contains("ActionView::ViewHelpers.html_escape(\"<h1>A mystery.\")"),
        "Rails escapes an unmarked html: body:\n{out}",
    );
}

#[test]
fn an_html_safe_body_is_not_escaped() {
    let out = controller_src("    render html: \"<h1>404</h1>\".html_safe\n");
    assert!(
        !out.contains("html_escape"),
        "a body marked safe at the render site must not be escaped:\n{out}",
    );
    assert!(
        !out.contains("html_safe"),
        "and the mark itself is consumed, not emitted — `String#html_safe` \
         is an ActiveSupport core ext no AOT tree has:\n{out}",
    );
    assert!(
        out.contains("render(\"<h1>404</h1>\""),
        "the body passes through bare:\n{out}",
    );
}

#[test]
fn a_raw_body_is_not_escaped() {
    let out = controller_src("    render html: raw(content)\n");
    assert!(
        !out.contains("html_escape"),
        "`raw(x)` marks the body safe just as `x.html_safe` does:\n{out}",
    );
    assert!(
        out.contains("render(content"),
        "the raw() argument passes through bare:\n{out}",
    );
}

#[test]
fn a_marked_body_behind_a_local_still_escapes() {
    // The mark has to be written AT the render. A value that became
    // safe elsewhere is a runtime fact no call site can read, and
    // staying conservative there is what keeps the escape correct.
    let out = controller_src("    body = something\n    render html: body\n");
    assert!(
        out.contains("ActionView::ViewHelpers.html_escape(body)"),
        "an unmarked local is escaped:\n{out}",
    );
}
