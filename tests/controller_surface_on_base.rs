//! The controller surface belongs to `ActionController::Base`.
//!
//! `params`, `render`, `redirect_to` and the rest were registered on
//! `ApplicationController` — the class an app's OWN controllers happen
//! to inherit from, not the class Rails defines them on. A controller
//! that does not descend from it therefore resolved nothing.
//!
//! roundhouse's own synthesized redirect controller is exactly such a
//! controller: `root to: redirect("/x")` builds one parented to
//! `ActionController::Base`, and its `redirect_to` read as a call
//! nothing knew — with a diagnostic carrying no span, so it could not
//! even be located in the source.

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::analyze::{diagnose, DiagnosticKind};
use roundhouse::ingest::ingest_app_from_tree;

fn app_from(files: &[(&str, &str)]) -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = files
        .iter()
        .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn unresolved(app: &roundhouse::App) -> Vec<String> {
    diagnose(app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::UnresolvedType { name: Some(name), .. } => {
                Some(name.as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

const SCHEMA: &str = "ActiveRecord::Schema.define do\n  create_table \"gauges\", force: :cascade do |t|\n    t.string \"label\", null: false\n  end\nend\n";
const APP_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\nend\n";
const APP_CTRL: &str = "class ApplicationController < ActionController::Base\nend\n";

#[test]
fn a_controller_that_skips_the_app_base_still_resolves_the_surface() {
    // Rails allows it, and roundhouse synthesizes one.
    let app = app_from(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APP_RECORD),
        ("app/controllers/application_controller.rb", APP_CTRL),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ActionController::Base\n  def index\n    redirect_to(\"/elsewhere\", status: 301)\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]);
    let unresolved = unresolved(&app);
    assert!(
        !unresolved.iter().any(|n| n == "redirect_to"),
        "`redirect_to` is on ActionController::Base; unresolved = {unresolved:?}"
    );
}

#[test]
fn the_synthesized_redirect_controller_resolves_its_own_call() {
    // The case that found this. `root to: redirect(…)` builds a
    // controller roundhouse owns, parented to ActionController::Base,
    // whose whole body is a `redirect_to`.
    let app = app_from(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APP_RECORD),
        ("app/controllers/application_controller.rb", APP_CTRL),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  def index\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  root to: redirect(\"/gauges\")\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]);
    let unresolved = unresolved(&app);
    assert!(
        !unresolved.iter().any(|n| n == "redirect_to"),
        "the synthesized controller resolves its own call; unresolved = {unresolved:?}"
    );
}

#[test]
fn an_app_controller_still_reaches_the_surface_through_its_parent() {
    // The guard: the ordinary chain — a controller, the app's base,
    // then the framework class — must keep working, since that is
    // every controller in every app.
    let app = app_from(&[
        ("db/schema.rb", SCHEMA),
        ("app/models/application_record.rb", APP_RECORD),
        ("app/controllers/application_controller.rb", APP_CTRL),
        (
            "app/controllers/gauges_controller.rb",
            "class GaugesController < ApplicationController\n  def index\n    @label = params[:label].to_s\n    redirect_to(\"/elsewhere\")\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/gauges\", to: \"gauges#index\"\nend\n",
        ),
    ]);
    let unresolved = unresolved(&app);
    for name in ["params", "redirect_to"] {
        assert!(
            !unresolved.iter().any(|n| n == name),
            "`{name}` still resolves through the parent; unresolved = {unresolved:?}"
        );
    }
}
