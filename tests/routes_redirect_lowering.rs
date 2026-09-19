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
        emitted.contains("redirect_to(\"/reports\", status: 301)"),
        "Rails' routing redirect answers 301; got:\n{emitted}"
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
        emitted.contains("redirect_to(\"/reports\", status: 302)"),
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
fn a_block_redirect_is_still_dropped_with_its_ledger_line() {
    // There is no literal to serve, so the #82 contract stands.
    let app = app_with(
        "  get \"/reports\", to: \"reports#index\"\n  get \"/old\", to: redirect { |params, request| \"/reports\" }\n",
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
