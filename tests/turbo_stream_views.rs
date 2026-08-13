//! `.turbo_stream.erb` templates — the format dimension.
//!
//! A Turbo form submission negotiates `text/vnd.turbo-stream.html`, and
//! Rails renders `<action>.turbo_stream.erb` for it. Those templates were
//! skipped at ingest with a "non-html format" ledger line, on the
//! grounds that their stems collide with the html template's on emit.
//! Format-qualified method names answer that: `create.turbo_stream.erb`
//! lowers to `create_turbo_stream`, sitting BESIDE `create` — the same
//! shape jbuilder's `_json` variants already use.
//!
//! The `turbo_stream.<action>` builder lowers onto
//! `Broadcasts.turbo_stream_fragment`, the composer each target's
//! hand-written Broadcasts already carries for the model-side
//! `broadcast_append_to` path — so the `<turbo-stream>` markup keeps ONE
//! owner per target rather than growing a second copy in the lowerer.

use roundhouse::app::App;
use roundhouse::dialect::LibraryClass;

fn app_with_template(body: &str) -> App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n",
        ),
        ("app/models/thing.rb", "class Thing < ApplicationRecord\nend\n"),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  def create\n    @thing = Thing.new\n  end\nend\n",
        ),
        (
            "app/views/things/_thing.html.erb",
            "<div id=\"<%= dom_id(thing) %>\"><%= thing.name %></div>\n",
        ),
        ("app/views/things/index.html.erb", "<h1>Things</h1>\n"),
        ("app/views/things/create.turbo_stream.erb", Box::leak(body.to_string().into_boxed_str())),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

fn view_lcs(app: &App) -> Vec<LibraryClass> {
    roundhouse::lower::lower_views_to_library_classes(&app.views, app, Vec::new())
}

fn method_body(app: &App, method: &str) -> String {
    for lc in view_lcs(app) {
        if let Some(m) = lc.methods.iter().find(|m| m.name.as_str() == method) {
            return format!("{:?}", m.body);
        }
    }
    panic!("no view method named {method}");
}

#[test]
fn a_turbo_stream_template_is_ingested_and_named_for_its_format() {
    let app = app_with_template("<%= turbo_stream.append \"things\", @thing %>\n");
    let formats: Vec<&str> = app.views.iter().map(|v| v.format.as_str()).collect();
    assert!(formats.contains(&"turbo_stream"), "got {formats:?}");

    // Beside `index`, not on top of it — the stem-collision objection
    // that kept non-html ERB out of the view path.
    let names: Vec<String> = view_lcs(&app)
        .iter()
        .flat_map(|lc| lc.methods.iter().map(|m| m.name.as_str().to_string()))
        .collect();
    assert!(names.contains(&"create_turbo_stream".to_string()), "got {names:?}");
    assert!(names.contains(&"index".to_string()), "got {names:?}");
}

#[test]
fn append_with_a_record_renders_that_records_partial() {
    let app = app_with_template("<%= turbo_stream.append \"things\", @thing %>\n");
    let body = method_body(&app, "create_turbo_stream");
    assert!(body.contains("turbo_stream_fragment"), "got {body}");
    assert!(body.contains("append"), "the action: {body}");
    // Rails renders the record's own partial for the `<template>`.
    assert!(body.contains("Things"), "the partial module: {body}");
    // Markup must NOT be escaped on its way to the buffer.
    assert!(!body.contains("html_escape"), "fragment was escaped: {body}");
}

#[test]
fn remove_targets_the_records_dom_id_and_carries_no_template() {
    let app = app_with_template("<%= turbo_stream.remove @thing %>\n");
    let body = method_body(&app, "create_turbo_stream");
    assert!(body.contains("turbo_stream_fragment"), "got {body}");
    assert!(body.contains("dom_id"), "a bare record target is its dom_id: {body}");
    // `remove` has no content, and the runtime composer omits the
    // `<template>` entirely for it.
    assert!(body.contains("remove"), "got {body}");
}

#[test]
fn an_explicit_target_expression_is_used_verbatim() {
    // Only a BARE RECORD becomes a dom_id; a string is already the id.
    let app = app_with_template("<%= turbo_stream.append \"messages\", @thing %>\n");
    let body = method_body(&app, "create_turbo_stream");
    assert!(body.contains("messages"), "got {body}");
}

#[test]
fn the_option_form_is_left_alone_rather_than_half_lowered() {
    // `partial:`/`collection:`/`locals:` needs the partial machinery a
    // `render` call site gets. Declining keeps the source shape (and
    // files a residue line at emit); half-lowering would look like it
    // worked.
    let app = app_with_template(
        "<%= turbo_stream.replace :box, partial: \"things/thing\", collection: @things %>\n",
    );
    let body = method_body(&app, "create_turbo_stream");
    assert!(
        !body.contains("turbo_stream_fragment"),
        "unsupported spelling must not be lowered: {body}"
    );
}
