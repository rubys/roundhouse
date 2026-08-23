//! `json.partial! partial: P, collection: C, as: V` — Jbuilder's own
//! alias for `json.array! C, partial: P, as: V`, and the spelling
//! campfire's autocomplete index uses.
//!
//! Four separate holes met in one template, each invisible until the
//! one before it was gone:
//!
//!   1. The `partial!` arm required a POSITIONAL path, so this call
//!      (whose first argument is the options hash) went unrecognized
//!      and the whole template lowered to an empty `{}`.
//!   2. The partial's module path was camelized WITHOUT splitting on
//!      `/`, so a nested dir emitted `Views::Autocompletable/users
//!      .user_json(user)` — which parses as division.
//!   3. The template's parameter came from a NAME GUESS
//!      (`autocompletable/users/index` -> `users`) where the body reads
//!      `@page`; def site and call site then disagreed about arity.
//!   4. `h(x)` was left as a bare call. In ERB it is unwrapped (the
//!      auto-escape wrapper does the same job); in JSON there is no
//!      wrapper, and `JsonBuilder.encode_value` escapes for JSON, not
//!      for HTML.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn tree(files: &[(&str, &str)]) -> HashMap<PathBuf, Vec<u8>> {
    files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect()
}

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "users", force: :cascade do |t|
    t.string "name", null: false
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  namespace :autocompletable do
    resources :users, only: :index
  end
end
"#;

const CONTROLLER: &str = r#"class Autocompletable::UsersController < ApplicationController
  def index
    @page = User.all
  end
end
"#;

const INDEX: &str =
    r#"json.partial! partial: "autocompletable/users/user", collection: @page, as: :user
"#;

const PARTIAL: &str = r#"json.name  h(user.name)
json.value user.id
"#;

fn emitted(suffix: &str) -> String {
    let mut app = ingest_app_from_tree(tree(&[
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/user.rb", "class User < ApplicationRecord\nend\n"),
        ("app/controllers/autocompletable/users_controller.rb", CONTROLLER),
        ("app/views/autocompletable/users/index.json.jbuilder", INDEX),
        ("app/views/autocompletable/users/_user.json.jbuilder", PARTIAL),
    ]))
    .expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let mut files = ruby::emit_lowered_jbuilder_views(&app);
    files.extend(ruby::emit_lowered_controllers(&app));
    files
        .iter()
        .find(|f| f.path.to_string_lossy().ends_with(suffix))
        .map(|f| f.content.clone())
        .unwrap_or_else(|| {
            panic!(
                "no emitted file ends with {suffix}; got {:?}",
                files.iter().map(|f| f.path.clone()).collect::<Vec<_>>()
            )
        })
}

#[test]
fn the_collection_form_lowers_to_an_array() {
    let src = emitted("autocompletable/users/index_json.rb");
    assert!(
        src.contains("io << \"[\""),
        "the collection form answers a top-level ARRAY:\n{src}"
    );
    assert!(
        src.contains(".map") && src.contains(".join(\",\")"),
        "one partial call per element:\n{src}"
    );
}

/// A nested partial dir names a nested module. Camelizing without
/// splitting on `/` emitted a division.
#[test]
fn the_partial_module_path_is_nested() {
    let src = emitted("autocompletable/users/index_json.rb");
    assert!(
        src.contains("Views::Autocompletable::Users.user_json"),
        "the partial resolves through its nested module:\n{src}"
    );
    assert!(
        !src.contains("Autocompletable/users"),
        "no slash survives into a constant path:\n{src}"
    );
}

/// The def site takes what the template READS, and the call site
/// passes it. Both come from the same survey, so they cannot disagree.
#[test]
fn the_parameter_is_the_ivar_the_template_reads() {
    let view = emitted("autocompletable/users/index_json.rb");
    assert!(
        view.contains("def self.index_json(page)"),
        "the parameter is the ivar, not the name-convention guess:\n{view}"
    );
    let ctrl = emitted("app/controllers/autocompletable/users_controller.rb");
    assert!(
        ctrl.contains("index_json(@page)"),
        "the render call site passes it:\n{ctrl}"
    );
}

/// `h` is a REAL escape in a JSON template — there is no auto-escape
/// wrapper to make it redundant, and `encode_value` escapes for JSON.
#[test]
fn h_escapes_for_html_in_a_json_value() {
    let src = emitted("autocompletable/users/_user_json.rb");
    assert!(
        src.contains("ActionView::ViewHelpers.html_escape(user.name)"),
        "`h(x)` becomes a real html_escape:\n{src}"
    );
}
