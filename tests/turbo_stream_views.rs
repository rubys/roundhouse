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
        (
            "app/models/thing.rb",
            "class Thing < ApplicationRecord\nend\n",
        ),
        (
            "app/controllers/things_controller.rb",
            "class ThingsController < ApplicationController\n  def create\n    @thing = Thing.new\n  end\nend\n",
        ),
        (
            "app/views/things/_thing.html.erb",
            "<div id=\"<%= dom_id(thing) %>\"><%= thing.name %></div>\n",
        ),
        ("app/views/things/index.html.erb", "<h1>Things</h1>\n"),
        (
            "app/views/things/create.turbo_stream.erb",
            Box::leak(body.to_string().into_boxed_str()),
        ),
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
    assert!(
        names.contains(&"create_turbo_stream".to_string()),
        "got {names:?}"
    );
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
    assert!(
        !body.contains("html_escape"),
        "fragment was escaped: {body}"
    );
}

#[test]
fn remove_targets_the_records_dom_id_and_carries_no_template() {
    let app = app_with_template("<%= turbo_stream.remove @thing %>\n");
    let body = method_body(&app, "create_turbo_stream");
    assert!(body.contains("turbo_stream_fragment"), "got {body}");
    assert!(
        body.contains("dom_id"),
        "a bare record target is its dom_id: {body}"
    );
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
fn rust_view_emission_qualifies_the_turbo_stream_runtime_composer() {
    let app = app_with_template("<%= turbo_stream.append \"things\", @thing %>\n");
    let emitted = roundhouse::emit::rust::emit(&app);
    let view = emitted
        .iter()
        .find(|file| {
            file.path.to_string_lossy().starts_with("src/views/")
                && file.content.contains("fn create_turbo_stream")
        })
        .unwrap_or_else(|| panic!("missing emitted turbo_stream view"));

    assert!(
        view.content.contains("crate::broadcasts::Broadcasts::turbo_stream_fragment("),
        "view must call the runtime composer explicitly: {}",
        view.content
    );
    assert!(
        !view.content.contains("= Broadcasts::turbo_stream_fragment("),
        "no ambiguous unqualified composer call should remain: {}",
        view.content
    );
}

fn create_action_body(app: &App) -> String {
    let lcs = roundhouse::lower::lower_controllers_with_arel_and_views(
        &app.controllers,
        Vec::new(),
        None,
        &app.views,
    );
    let lc = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "ThingsController")
        .expect("ThingsController");
    let m = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "create")
        .expect("create");
    format!("{:?}", m.body)
}

#[test]
fn the_action_dispatches_on_the_request_format() {
    let app = app_with_template("<%= turbo_stream.append \"things\", @thing %>\n");
    let body = create_action_body(&app);
    assert!(
        body.contains("turbo_stream"),
        "expected a format branch: {body}"
    );
    assert!(
        body.contains("create_turbo_stream"),
        "the turbo_stream branch renders the format-qualified view: {body}"
    );
    assert!(
        body.contains("text/vnd.turbo-stream.html"),
        "Turbo ignores a response typed text/html: {body}"
    );
}

#[test]
fn the_html_branch_raises_missing_template_when_there_is_no_html_view() {
    // The action has ONLY a turbo_stream template, which is exactly
    // campfire's `MessagesController#create`. Rails raises
    // MissingTemplate for an html request there; emitting a call to a
    // `Views::Things.create` that was never generated would NameError
    // instead. Letting turbo_stream views into the contract map under
    // the bare stem is what used to cause that.
    let app = app_with_template("<%= turbo_stream.append \"things\", @thing %>\n");
    let body = create_action_body(&app);
    assert!(body.contains("MissingTemplate"), "got {body}");
}

#[test]
fn a_bare_accept_any_renders_the_template_the_action_has() {
    // The same turbo_stream-only action reached by campfire's attachment
    // uploader: an XMLHttpRequest that sets no Accept, so `*/*`. Rails
    // expands `*/*` to every format, finds `create.turbo_stream.erb`,
    // and answers 200; the upload counts anything under 400 as done.
    // The html fallback asks `accepts_any_format` before it raises.
    let app = app_with_template("<%= turbo_stream.append \"things\", @thing %>\n");
    let body = create_action_body(&app);
    let any = body
        .find("accepts_any_format")
        .unwrap_or_else(|| panic!("no any-format arm: {body}"));
    let raise = body
        .find("MissingTemplate")
        .unwrap_or_else(|| panic!("no raise: {body}"));
    assert!(
        any < raise,
        "the any-format arm is asked before the raise: {body}"
    );
    assert!(
        body[any..raise].contains("create_turbo_stream"),
        "the any-format arm renders the turbo_stream view: {body}"
    );
}

#[test]
fn the_option_form_with_an_unknown_key_is_left_alone() {
    // A key this pass does not read (`layout:`) stays source-shaped.
    // Half-lowering would look like it worked and drop the unread option.
    let app = app_with_template(
        "<%= turbo_stream.replace :box, partial: \"things/thing\", layout: \"box\" %>\n",
    );
    let body = method_body(&app, "create_turbo_stream");
    assert!(
        !body.contains("turbo_stream_fragment"),
        "unsupported spelling must not be lowered: {body}"
    );
}

/// `collection:` is Rails' once-per-element render. campfire's
/// `accounts/users/index.turbo_stream.erb` writes
/// `turbo_stream.replace :next_page_container, partial: "…/user",
/// collection: @page.records, as: :user`. Treating that as a single
/// render would keep only the first element.
#[test]
fn the_collection_option_form_renders_each_element() {
    let app = app_with_template(
        "<%= turbo_stream.replace :box, partial: \"things/thing\", collection: @things, as: :thing %>\n",
    );
    let body = method_body(&app, "create_turbo_stream");
    assert!(body.contains("turbo_stream_fragment"), "got {body}");
    assert!(body.contains("each"), "the collection must iterate: {body}");
    assert!(
        body.contains("_ts_cap"),
        "concatenated into the fragment: {body}"
    );
    assert!(
        body.contains("Views::Things") && body.contains("thing"),
        "the named partial supplies each element: {body}"
    );
    assert!(
        body.contains("BoolOp") || body.contains("Array {"),
        "a nil collection must iterate as empty, not as nil.each: {body}"
    );
}

/// A supplied `as:` that is not a name literal cannot be lowered —
/// dropping it would bind the element to the partial's default name.
#[test]
fn an_invalid_as_option_stays_source_shaped() {
    let app = app_with_template(
        "<%= turbo_stream.replace :box, partial: \"things/thing\", collection: @things, as: @name %>\n",
    );
    let body = method_body(&app, "create_turbo_stream");
    assert!(
        !body.contains("turbo_stream_fragment"),
        "invalid as: must not lower: {body}"
    );
}

/// `locals:` evaluate once at the call site, even for an empty
/// collection. Binding them inside `each` would skip or repeat them.
#[test]
fn collection_locals_are_bound_before_each() {
    let app = app_with_template(
        "<%= turbo_stream.replace :box, partial: \"things/thing\", collection: @things, as: :thing, locals: { heading: next_heading } %>\n",
    );
    let body = method_body(&app, "create_turbo_stream");
    assert!(body.contains("_ts_local_"), "locals bound once: {body}");
    assert!(body.contains("each"), "still iterates: {body}");
}

/// BLOCK form: `turbo_stream.append target do … end`. The block body is
/// the `<template>` content, so it walks into its own capture
/// accumulator and the captured String reaches the same composer — the
/// `<turbo-stream>` markup keeps one owner.
///
/// campfire's `rooms/refreshes/show.turbo_stream.erb` opens with it, and
/// until this landed the call was left in source shape, where
/// `turbo_stream` resolves to nothing at all.
#[test]
fn the_block_form_captures_its_body_as_the_template() {
    let app = app_with_template(
        "<%= turbo_stream.append \"things\" do %>\n  <%= render partial: \"things/thing\", collection: @things %>\n<% end %>\n",
    );
    let body = method_body(&app, "create_turbo_stream");
    assert!(
        body.contains("turbo_stream_fragment"),
        "the block form must reach the composer: {body}"
    );
    assert!(
        body.contains("_ts_cap"),
        "with its own capture accumulator: {body}"
    );
    assert!(
        !body.contains("html_escape"),
        "a turbo-stream fragment is markup, not escaped text: {body}"
    );
}

/// OPTION form, the half that is safe to lower: `partial:` names one
/// template and `locals:` binds it. Resolved through the same
/// `named_partial_call` a `render partial:` site uses, so there is only
/// one set of partial rules.
#[test]
fn the_partial_option_form_renders_that_partial() {
    let app = app_with_template(
        "<%= turbo_stream.replace :box, partial: \"things/thing\", locals: { thing: @thing } %>\n",
    );
    let body = method_body(&app, "create_turbo_stream");
    assert!(body.contains("turbo_stream_fragment"), "got {body}");
    assert!(body.contains("replace"), "the action: {body}");
    assert!(
        body.contains("Views::Things") && body.contains("thing"),
        "the named partial supplies the template: {body}"
    );
}

#[test]
fn a_dynamic_frame_src_is_evaluated_once_in_rust_views() {
    let app = app_with_template(
        "<%= turbo_frame_tag :next_page_container, src: things_path(page: @thing.name) %>\n",
    );
    let emitted = roundhouse::emit::rust::emit(&app);
    let view = emitted
        .iter()
        .find(|file| {
            file.path.to_string_lossy().starts_with("src/views/")
                && file.content.contains("fn create_turbo_stream")
        })
        .expect("generated turbo_stream view");

    assert!(
        view.content.contains("let __roundhouse_compacted_attr_"),
        "dynamic compacted attribute should bind its computed value once:\n{}",
        view.content
    );
    assert_eq!(
        view.content.matches("things_path(").count(),
        1,
        "the path helper must not be evaluated separately for the nil check and render:\n{}",
        view.content
    );
}
