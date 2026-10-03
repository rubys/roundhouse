//! `root to: redirect("/scan")` — the routing-level redirect, served.
//!
//! #82 stopped it from emitting an invalid route and gave the drop a
//! ledger line. This is the other half rubys named there: "a
//! compile-time 301 is cheap to serve on every target, but it needs a
//! route kind that isn't (controller, action) across a dozen emitters".
//!
//! It does not, if the route points at an action instead. The literal
//! redirect lowers to the shape an app writes by hand for the same
//! thing — a controller action calling `redirect_to`, which every
//! target already serves — so no emitter learns a new route kind.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

fn app_with(routes: &str) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"reports\", force: :cascade do |t|\n    t.string \"name\", null: false\n  end\nend\n".to_string(),
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\nend\n".to_string()),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  before_action :authenticate\n\n  private\n\n  def authenticate\n    head :unauthorized\n  end\nend\n".to_string(),
        ),
        (
            "app/controllers/reports_controller.rb",
            "class ReportsController < ApplicationController\n  def index; end\nend\n".to_string(),
        ),
        ("config/routes.rb", format!("Rails.application.routes.draw do\n{routes}end\n")),
    ]
    .into_iter()
    .map(|(p, c)| (PathBuf::from(p), c.into_bytes()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn redirect_controller(app: &roundhouse::App) -> String {
    ruby::emit_lowered_controllers(app)
        .into_iter()
        .find(|f| f.path.display().to_string().ends_with("roundhouse_redirects_controller.rb"))
        .map(|f| f.content)
        .expect("the redirect controller is emitted")
}

#[test]
fn a_root_redirect_is_served_by_a_synthesized_action() {
    let app = app_with("  get \"/reports\", to: \"reports#index\"\n  root to: redirect(\"/reports\")\n");
    let emitted = redirect_controller(&app);
    assert!(
        emitted.contains("redirect_to(\"/reports\", status: :moved_permanently)"),
        "Rails' routing redirect answers 301, passed in the Symbol form \
         `resolve_status` takes; got:\n{emitted}"
    );
    // The route table points at the action, so no emitter needs a route
    // kind for the redirect itself.
    assert!(
        app.routes.entries.iter().any(|e| matches!(
            e,
            roundhouse::dialect::RouteSpec::Explicit { controller, path, .. }
                if controller.0.as_str() == "RoundhouseRedirectsController" && path == "/"
        )),
        "routes = {:?}",
        app.routes.entries
    );
}

#[test]
fn the_synthesized_controller_stays_out_of_the_apps_filter_stack() {
    // A routing redirect never enters the controller stack, so it must
    // not pick up `ApplicationController`'s filters — the fixture
    // authenticates there, and the redirect has to answer anyway.
    let app = app_with("  get \"/reports\", to: \"reports#index\"\n  root to: redirect(\"/reports\")\n");
    let emitted = redirect_controller(&app);
    assert!(
        emitted.contains("class RoundhouseRedirectsController < ActionController::Base"),
        "got:\n{emitted}"
    );
    assert!(!emitted.contains("authenticate"), "got:\n{emitted}");
}

#[test]
fn an_explicit_verb_redirect_carries_its_status_and_its_own_action() {
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  get \"/admin\", to: redirect(\"/reports\")\n  get \"/old\", to: redirect(\"/reports\", status: 302)\n  root to: redirect(\"/reports\")\n",
    );
    let emitted = redirect_controller(&app);
    assert!(emitted.contains("def admin"), "one action per route; got:\n{emitted}");
    assert!(emitted.contains("def old"), "got:\n{emitted}");
    assert!(emitted.contains("def root"), "got:\n{emitted}");
    assert!(
        emitted.contains("redirect_to(\"/reports\", status: :found)"),
        "an explicit `status:` is carried; got:\n{emitted}"
    );
}

#[test]
fn a_redirect_inside_a_namespace_keeps_the_one_controller() {
    // `qualify_controller` would otherwise make it
    // `Admin::RoundhouseRedirectsController`, a class nobody defines.
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  namespace :admin do\n    get \"/\", to: redirect(\"/reports\")\n  end\n",
    );
    let emitted = redirect_controller(&app);
    assert!(emitted.contains("class RoundhouseRedirectsController"), "got:\n{emitted}");
    assert!(!emitted.contains("Admin::Roundhouse"), "got:\n{emitted}");
}

#[test]
fn a_multiline_status_redirect_is_served() {
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  get \"old_claims\", to: redirect(status: 301) { |_, request|\n    qs = request.query_string\n    qs.present? ? \"/reports?#{qs}\" : \"/reports\"\n  }\n",
    );
    let emitted = redirect_controller(&app);
    assert!(emitted.contains("def old_claims"), "multiline redirect; got:\n{emitted}");
    assert!(emitted.contains("reports"), "present? path; got:\n{emitted}");
    assert!(emitted.contains("status: :moved_permanently"), "status 301; got:\n{emitted}");
}

#[test]
fn a_block_that_builds_a_string_is_served() {
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  get \"/old\", to: redirect(status: 301) { |params, request| request.query_string.empty? ? \"/reports\" : \"/reports?#{request.query_string}\" }\n  get \"/older\", to: redirect { |_| \"/reports\" }\n  get \"/one\", to: redirect { |params| \"/reports/#{params[:id]}\" }\n  get \"/claim\", to: redirect(status: 301) { |_, request| qs = request.query_string; qs.present? ? \"/reports?#{qs}\" : \"/reports\" }\n  get \"/parks\", to: redirect { |params, req| query = req.query_string.empty? ? \"\" : \"?#{req.query_string}\"; \"/items/#{params[:item_slug]}#{query}\" }\n  get \"/not_a_string\", to: redirect(status: 301) { |_, request| request.user }\n",
    );
    let emitted = redirect_controller(&app);
    assert!(emitted.contains("query_string"), "built path; got:\n{emitted}");
    assert!(emitted.contains("def older"), "unused parameter; got:\n{emitted}");
    assert!(emitted.contains("params[:id]") || emitted.contains("@params"), "one-arg interpolation; got:\n{emitted}");
    assert!(
        emitted.contains("reports") && emitted.contains("strip.empty?"),
        "multi-statement present?; got:\n{emitted}"
    );
    assert!(
        emitted.contains("def claim") && emitted.contains("status: :moved_permanently"),
        "status beside the block survived; got:\n{emitted}"
    );
    assert!(
        !app.routes.entries.iter().any(|entry| format!("{entry:?}").contains("/not_a_string")),
        "a non-string block stays dropped"
    );
    assert!(emitted.contains("item_slug"), "req and params[]; got:\n{emitted}");
    assert!(emitted.contains("request.query_string"), "req renamed; got:\n{emitted}");
}

#[test]
fn a_string_block_redirect_is_served() {
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  get \"/old\", to: redirect { |params, request| \"/reports\" }\n  get \"/older\", to: redirect { |request| \"/reports\" }\n  root to: redirect(\"/reports\")\n",
    );
    let emitted = redirect_controller(&app);
    assert!(emitted.contains("def old"), "one-arg block redirect; got:\n{emitted}");
    assert!(emitted.contains("def older"), "two-arg block redirect; got:\n{emitted}");
    assert!(emitted.contains("redirect_to(\"/reports\", status: :moved_permanently)"), "{emitted}");
}

#[test]
fn via_and_regexp_constraints_stay_on_a_string_target() {
    let app = app_with(
        "  match \"/reports\", to: \"reports#index\", via: %i[get post delete], constraints: { id: /\\d+/ }\n",
    );
    let methods: Vec<_> = app.routes.entries.iter().filter_map(|entry| match entry {
        roundhouse::dialect::RouteSpec::Explicit { method, constraints, .. } => Some((format!("{method:?}"), constraints.len())),
        roundhouse::dialect::RouteSpec::Scope { entries, .. } => {
            assert!(entries.len() >= 3, "{entries:?}");
            None
        }
        _ => None,
    }).collect();
    assert!(methods.len() >= 3 || app.routes.entries.iter().any(|entry| matches!(entry, roundhouse::dialect::RouteSpec::Scope { entries, .. } if entries.len() >= 3)), "{:?}", app.routes.entries);
}

#[test]
fn engine_routes_stay_explicit_gaps() {
    let mounted = app_with("  mount Sidekiq::Web, at: \"/sidekiq\"\n");
    assert!(!format!("{:?}", mounted.routes.entries).contains("Sidekiq"));
    let err = ingest_app_from_tree({
        let mut tree = std::collections::HashMap::new();
        tree.insert(std::path::PathBuf::from("config/routes.rb"), b"Rails.application.routes.draw do\n  use_doorkeeper\nend\n".to_vec());
        tree.insert(std::path::PathBuf::from("app/controllers/application_controller.rb"), b"class ApplicationController < ActionController::Base\nend\n".to_vec());
        tree
    });
    assert!(err.expect_err("use_doorkeeper").to_string().contains("use_doorkeeper"));
}

#[test]
fn a_block_redirect_is_still_dropped_with_its_ledger_line() {
    // There is no literal to serve, so the #82 contract stands.
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  get \"/old\", to: redirect { |params, request| request.user }\n",
    );
    assert!(
        !app.routes.entries.iter().any(|e| matches!(
            e,
            roundhouse::dialect::RouteSpec::Explicit { path, .. } if path == "/old"
        )),
        "routes = {:?}",
        app.routes.entries
    );
}

#[test]
fn a_path_placeholder_is_filled_from_the_matched_params() {
    // Lobsters: `get "/u/:username", to: redirect("/~%{username}",
    // status: 301)`. Emitted literally, every old profile link answered
    // a Location of `/~%{username}`.
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  get \"/u/:username\", to: redirect(\"/~%{username}\", status: 301)\n",
    );
    let emitted = redirect_controller(&app);
    assert!(!emitted.contains("%{username}"), "got:\n{emitted}");
    assert!(
        emitted.contains("redirect_to(\"/~#{@params[\"username\"]}\", status: :moved_permanently)"),
        "got:\n{emitted}"
    );
}
