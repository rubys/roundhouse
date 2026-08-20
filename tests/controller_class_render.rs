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

fn app_with(partial_body: &str, call: &str) -> App {
    let helper = format!(
        "require \"test_helper\"\n\nclass UserTest < ActiveSupport::TestCase\n  test \"renders\" do\n    frag = {call}\n    assert frag\n  end\nend\n"
    );
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
        ("app/views/users/_mention.html.erb", Box::leak(partial_body.to_string().into_boxed_str())),
        ("app/views/users/show.html.erb", "<%= render partial: \"users/mention\", locals: { user: @user } %>\n"),
        ("test/models/user_test.rb", Box::leak(helper.into_boxed_str())),
    ]))
    .expect("ingest class-render app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn test_ir(partial_body: &str, call: &str) -> String {
    let app = app_with(partial_body, call);
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
