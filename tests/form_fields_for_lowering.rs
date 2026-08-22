//! `form.fields_for :settings, obj do |nested| … end` — Rails' nested
//! object-name scope, macro-inlined like every other builder method.
//!
//! It is the one builder method that renders NO markup of its own: it
//! binds a SECOND builder whose object name is `parent[nested]`, and the
//! fields inside it name `account[settings][x]` and id
//! `account_settings_x` (Rails derives the id from the bracketed name —
//! `Tags::Base#sanitized_object_name`, transcribed into `field_id`).
//!
//! Unhandled, it was not merely unsupported: `form_with` is
//! macro-inlined, so no `form` local exists in the emitted view, and an
//! unrecognized `form.<x>` call survived as a bare `form.fields_for(…)`
//! — `undefined local variable or method 'form'`, a NameError on a page
//! that otherwise rendered.

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::lower_view_to_library_class;

fn lower_view(files: Vec<(&str, &str)>) -> String {
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let app = ingest_app_from_tree(tree).expect("ingest tree");
    let view = app.views.first().expect("view ingested");
    let lc = lower_view_to_library_class(view, &app);
    format!("{:?}", lc.methods.first().expect("view method").body)
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define(version: 1) do
  create_table :accounts do |t|
    t.string :name
    t.string :settings
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  resource :account
end
"#;

fn body(view: &'static str) -> String {
    lower_view(vec![
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/account.rb", "class Account < ApplicationRecord\nend\n"),
        ("app/views/accounts/edit.html.erb", view),
    ])
}

const NESTED: &str = r#"<%= form_with model: @account do |form| %>
  <%= form.fields_for :settings, @account.settings do |settings_form| %>
    <%= settings_form.hidden_field :quiet, value: "1" %>
  <% end %>
<% end %>
"#;

#[test]
fn the_nested_field_names_through_the_parent() {
    let b = body(NESTED);
    assert!(
        b.contains("account[settings][quiet]"),
        "the nested builder's object name is `parent[nested]`:\n{b}"
    );
}

/// Rails renders the ID from the same object name with the brackets
/// substituted — `account[settings]` → `account_settings`. A raw
/// bracketed id is not a legal HTML id and is not what Rails writes.
#[test]
fn the_nested_field_ids_with_underscores() {
    let b = body(NESTED);
    assert!(
        b.contains("account_settings_quiet"),
        "the id is the sanitized object name plus the field:\n{b}"
    );
    assert!(
        !b.contains("id=\\\"account[settings]"),
        "no bracketed id survives:\n{b}"
    );
}

/// The whole reason this is a lowering and not a runtime call: nothing
/// named `form` or `settings_form` exists in the emitted view.
#[test]
fn no_builder_local_survives_into_the_emit() {
    let b = body(NESTED);
    assert!(
        !b.contains("\"fields_for\""),
        "the call is expanded, not emitted:\n{b}"
    );
    assert!(
        !b.contains("\"hidden_field\""),
        "the nested builder's own calls expand too:\n{b}"
    );
}

/// `fields_for` opens no tag of its own — the nested fields land in the
/// SAME `<form>`, and a stray `</fields>`-shaped close would be markup
/// Rails never writes.
#[test]
fn it_renders_no_wrapper_of_its_own() {
    let b = body(NESTED);
    assert_eq!(
        b.matches("</form>").count(),
        1,
        "exactly one form close, from form_with:\n{b}"
    );
}
