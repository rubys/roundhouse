//! `resources :x, constraints: { id: /.../ }` — rubys/roundhouse#857.
//!
//! The inline `constraints:` kwarg on `resources`/`resource` was parsed
//! correctly by `ingest_explicit_route` (an explicit `get`/`post` route)
//! and by the `constraints(...) do … end` BLOCK wrapper around a
//! `resources` call (`merge_outer_constraints`), but
//! `ingest_resources_route`'s own kwarg loop fell through every
//! unrecognized key, `constraints:` included, to a silent `_ => {}` —
//! the resource was always built with `constraints: IndexMap::new()`.
//! A member route (`POST /widgets/:id/`, from a `member do … end`
//! block) therefore carried no requirement on `:id` and matched a
//! sibling collection path Rails routes elsewhere (`POST
//! /widgets/callback`), because the member route is registered first
//! and nothing disqualified `"callback"` as an `:id`.
//!
//! Verified against Rails 8.1.4 (`actionpack` 8.1.4, run locally):
//! `resources :widgets, only: %i[show], constraints: { id: /[A-Z][A-Z]\d+/
//! } do … end` makes Rails apply `[A-Z][A-Z]\d+` to every route carrying
//! an `:id` segment in that resource's scope — the member route, the
//! implicit `show`, AND (confirmed by running
//! `ActionDispatch::Routing::RouteSet#recognize_path` against actual
//! Rails, not assumed) a NESTED resource's own `:id`, since Rails
//! constraints merge by scope and param name regardless of nesting
//! depth. `only:` and the inline kwarg compose exactly as they do
//! without `constraints:`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::dialect::HttpMethod;
use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::lower::routes::{FlatRoute, flatten_routes};

const ROUTES: &str = "\
Rails.application.routes.draw do
  resources :widgets, only: %i[show], constraints: { id: /[A-Z][A-Z]\\d+/ } do
    member do
      post '/', to: 'widgets#check', as: 'check'
    end
    collection do
      post :callback
    end
    resources :parts, only: %i[show]
  end
end
";

fn tree() -> HashMap<PathBuf, Vec<u8>> {
    let mut tree = HashMap::new();
    tree.insert(PathBuf::from("config/routes.rb"), ROUTES.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("db/schema.rb"),
        b"ActiveRecord::Schema[7.1].define(version: 1) do\nend\n".to_vec(),
    );
    tree
}

fn flat() -> Vec<FlatRoute> {
    let app = ingest_app_from_tree(tree()).expect("ingest tree");
    flatten_routes(&app)
}

fn find<'a>(routes: &'a [FlatRoute], method: HttpMethod, path: &str) -> &'a FlatRoute {
    routes
        .iter()
        .find(|r| r.method == method && r.path == path)
        .unwrap_or_else(|| panic!("missing {method:?} {path}: {routes:?}"))
}

/// The inline kwarg reaches the `member do … end` route this resource
/// generates — the exact divergence #857 opened on.
#[test]
fn inline_constraint_reaches_the_member_route() {
    let r = flat();
    let check = find(&r, HttpMethod::Post, "/widgets/:id/");
    assert_eq!(
        check.constraints,
        vec![("id".to_string(), "[A-Z][A-Z]\\d+".to_string())],
        "the resource's own constraints: must ride its member route: {:?}",
        check.constraints
    );
}

/// Same requirement on the implicit `show` (`only: %i[show]` keeps only
/// this one of the seven scaffold actions).
#[test]
fn inline_constraint_reaches_the_implicit_show_route() {
    let r = flat();
    let show = find(&r, HttpMethod::Get, "/widgets/:id");
    assert_eq!(
        show.constraints,
        vec![("id".to_string(), "[A-Z][A-Z]\\d+".to_string())],
        "show must inherit the resource's own :id constraint: {:?}",
        show.constraints
    );
}

/// The `collection do … end` route has no `:id` segment at all, so the
/// resource's `id:` constraint must not land on it (same rule the
/// block-form test `block_constraint_propagates_into_resources_member_
/// actions_only` already established for the wrapper form).
#[test]
fn inline_constraint_does_not_reach_the_collection_route() {
    let r = flat();
    let callback = find(&r, HttpMethod::Post, "/widgets/callback");
    assert!(
        callback.constraints.is_empty(),
        "collection route has no :id segment, so it carries no constraint: {:?}",
        callback.constraints
    );
}

/// Rails merges `constraints:` by scope and param name, so a nested
/// resource's own `:id` (same key, a different path segment) inherits
/// the enclosing resource's requirement too — confirmed against real
/// Rails 8.1.4 above. `merge_outer_constraints` already recurses this
/// way for the `constraints(...) do … end` block form; the fix reuses
/// it for the inline kwarg.
#[test]
fn inline_constraint_leaks_into_a_nested_resources_own_id() {
    let r = flat();
    let part_show = find(&r, HttpMethod::Get, "/widgets/:widget_id/parts/:id");
    assert_eq!(
        part_show.constraints,
        vec![("id".to_string(), "[A-Z][A-Z]\\d+".to_string())],
        "nested resource's own :id inherits the parent's inline constraint, matching Rails: {:?}",
        part_show.constraints
    );
}

const WIDGETS_CONTROLLER: &str = "\
class WidgetsController < ApplicationController
  def show
    render plain: \"show:#{params[:id]}\"
  end

  def check
    render plain: \"check:#{params[:id]}\"
  end

  def callback
    render plain: \"callback\"
  end
end
";

const PARTS_CONTROLLER: &str = "\
class PartsController < ApplicationController
  def show
    render plain: \"part:#{params[:widget_id]}:#{params[:id]}\"
  end
end
";

fn widgets_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER)
        .write("app/controllers/parts_controller.rb", PARTS_CONTROLLER)
        .write("config/routes.rb", ROUTES)
}

/// The emitted app actually dispatches the way Rails 8.1.4 does (run
/// and confirmed locally, see the module doc): the member route's
/// `:id` constraint rejects `"callback"`, so the request falls through
/// to the collection route instead of being swallowed by `check`.
#[test]
fn emitted_app_dispatches_like_rails() {
    widgets_app()
        .run_ruby(
            r#"
ActionController::Base.allow_forgery_protection = false

def req(method, path)
  Main.run_rack(
    "REQUEST_METHOD" => method,
    "PATH_INFO" => path,
    "QUERY_STRING" => "",
    "rack.input" => StringIO.new(""),
  )
end

status, _h, body = req("POST", "/widgets/callback")
raise "callback status #{status} body #{body.inspect}" unless status == 200 && body.join == "callback"

status, _h, body = req("POST", "/widgets/AB123/")
raise "check status #{status} body #{body.inspect}" unless status == 200 && body.join == "check:AB123"

status, _h, body = req("GET", "/widgets/AB123")
raise "show status #{status} body #{body.inspect}" unless status == 200 && body.join == "show:AB123"

status, _h, body = req("GET", "/widgets/AB123/parts/CD456")
raise "part status #{status} body #{body.inspect}" unless status == 200 && body.join == "part:AB123:CD456"

status, _h, _body = req("GET", "/widgets/AB123/parts/1")
raise "constrained nested id should 404, got #{status}" unless status == 404

status, _h, _body = req("GET", "/widgets/callback")
raise "show must not accept a non-matching :id, got #{status}" unless status == 404

puts "ALL OK"
"#,
        )
        .assert_passes();
}
