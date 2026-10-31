//! `ApplicationController.render partial: "users/mention", locals: {
//! user: user }`.
//!
//! Rails' class-side renderer: a controller can render a template with
//! no request, which is how a TEST builds a fragment to compare against
//! (campfire's `mention_attachment_for` embeds the rendered mention
//! inside an `<action-text-attachment>`). It is the same partial the
//! views render, reached from outside a view.
//!
//! Bound through the DEF SITE's own contract — the one a `render
//! partial:` call site in a view binds against — so the two cannot
//! disagree about what the partial takes.

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

fn app_with(partial: &str, partial_body: &str, call: &str) -> App {
    let helper = format!(
        "require \"test_helper\"\n\nclass UserTest < ActiveSupport::TestCase\n  test \"renders\" do\n    frag = {call}\n    assert frag\n  end\nend\n"
    );
    let partial_path = format!("app/views/users/_{partial}.html.erb");
    let show = format!("<%= render partial: \"users/{partial}\", locals: {{ user: @user }} %>\n");
    let mut app = ingest_app_from_tree(tree(&[
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :users do |t|\n    t.string :name\n  end\nend\n",
        ),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        (
            "app/controllers/users_controller.rb",
            "class UsersController < ApplicationController\n  def show\n    @user = User.first\n  end\nend\n",
        ),
        (partial_path.as_str(), partial_body),
        ("app/views/users/show.html.erb", show.as_str()),
        ("test/models/user_test.rb", Box::leak(helper.into_boxed_str())),
    ]))
    .expect("ingest class-render app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn test_ir(partial_body: &str, call: &str) -> String {
    ir_of(&app_with("mention", partial_body, call))
}

fn ir_of(app: &App) -> String {
    app.test_modules
        .iter()
        .flat_map(|m| m.tests.iter().map(|t| format!("{:?}", t.body)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A partial that needs only its record binds to the view-module call.
#[test]
fn a_record_only_partial_binds_to_its_view_module() {
    let ir = test_ir(
        "<span><%= user.name %></span>\n",
        "ApplicationController.render partial: \"users/mention\", locals: { user: User.first }",
    );
    assert!(ir.contains("Symbol(\"Views\")"), "the view module: {ir}");
    assert!(ir.contains("Symbol(\"mention\")"), "the partial's method: {ir}");
    assert!(
        !ir.contains("Symbol(\"render\")"),
        "no class-side render may survive: {ir}"
    );
}

/// A partial that reads an ivar the caller cannot supply DECLINES —
/// there is nothing to bind, and passing the wrong arguments silently
/// is worse than the NoMethodError this leaves.
#[test]
fn a_partial_needing_more_than_its_record_declines() {
    let ir = test_ir(
        "<span><%= user.name %> <%= @account.name %></span>\n",
        "ApplicationController.render partial: \"users/mention\", locals: { user: User.first }",
    );
    assert!(
        ir.contains("Symbol(\"render\")"),
        "an unbindable partial must be left alone: {ir}"
    );
}

/// An option this pass does not read leaves the call alone rather than
/// dropping it.
#[test]
fn an_unread_option_declines() {
    let ir = test_ir(
        "<span><%= user.name %></span>\n",
        "ApplicationController.render partial: \"users/mention\", locals: { user: User.first }, formats: :html",
    );
    assert!(ir.contains("Symbol(\"render\")"), "left alone: {ir}");
}

/// Rails' class-side renderer renders through a controller of its own,
/// with no request behind it: the call is bracketed the way a broadcast
/// render is, so `controller` inside it is the detached controller.
#[test]
fn a_class_side_render_takes_the_renderer_bracket() {
    let ir = test_ir(
        "<span><%= user.name %></span>\n",
        "ApplicationController.render partial: \"users/mention\", locals: { user: User.first }",
    );
    assert!(ir.contains("Symbol(\"begin_broadcast_render\")"), "{ir}");
    assert!(ir.contains("Symbol(\"broadcast_render\")"), "{ir}");
}

/// `collection:` renders the partial once per element, bound to its
/// record local, concatenated. A literal `cached:` is the renderer
/// controller's to answer, and the detached one caches nothing.
#[test]
fn a_collection_render_maps_the_partial_over_its_elements() {
    let ir = test_ir(
        "<span><%= user.name %></span>\n",
        "ApplicationController.render partial: \"users/mention\", collection: [ User.first ], cached: true",
    );
    assert!(ir.contains("Symbol(\"map\")") && ir.contains("Symbol(\"join\")"), "{ir}");
    assert!(ir.contains("Symbol(\"mention\")"), "{ir}");
    assert!(!ir.contains("Symbol(\"render\")"), "no class-side render may survive: {ir}");
}

/// A computed `cached:` is a key this does not read: left alone.
#[test]
fn a_computed_collection_cache_key_declines() {
    let ir = test_ir(
        "<span><%= user.name %></span>\n",
        "ApplicationController.render partial: \"users/mention\", collection: [ User.first ], cached: ->(u) { u }",
    );
    assert!(ir.contains("Symbol(\"render\")"), "left alone: {ir}");
}

/// The call names the partial by the method its definition has, not by
/// its stem: `_new` defines `_new` (`view_method_name`).
#[test]
fn a_partial_named_new_is_called_by_its_defined_method() {
    let ir = ir_of(&app_with(
        "new",
        "<span><%= user.name %></span>\n",
        "ApplicationController.render partial: \"users/new\", locals: { user: User.first }",
    ));
    assert!(ir.contains("Symbol(\"_new\")"), "the partial's defined method: {ir}");
    assert!(!ir.contains("Symbol(\"render\")"), "no class-side render may survive: {ir}");
}
