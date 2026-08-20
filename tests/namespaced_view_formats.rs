//! A NAMESPACED controller's non-html templates.
//!
//! `json_actions_for` / `turbo_stream_actions_for` turn the controller's
//! view module back into the directory a view name is keyed by, and both
//! used `snake_case`. That leaves the `::` alone: `Rooms::Refreshes`
//! became `rooms::refreshes`, which prefixes nothing, so a namespaced
//! controller's json and turbo_stream templates were INVISIBLE — the
//! action fell through to the html branch and raised MissingTemplate.
//! `underscore` is the function that answers `rooms/refreshes`.
//!
//! Top-level controllers were unaffected (no `::` to leave alone), which
//! is why the existing turbo_stream coverage passed throughout.

use roundhouse::app::App;

fn app() -> App {
    let files: Vec<(&str, &str)> = vec![
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :things do |t|\n    t.string :name\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  namespace :rooms do\n    resource :refresh, only: :show\n  end\nend\n",
        ),
        ("app/models/thing.rb", "class Thing < ApplicationRecord\nend\n"),
        (
            "app/controllers/rooms/refreshes_controller.rb",
            "class Rooms::RefreshesController < ApplicationController\n  def show\n    @thing = Thing.new\n  end\nend\n",
        ),
        (
            "app/views/things/_thing.html.erb",
            "<div id=\"<%= dom_id(thing) %>\"><%= thing.name %></div>\n",
        ),
        (
            "app/views/rooms/refreshes/show.turbo_stream.erb",
            "<%= turbo_stream.append \"things\", @thing %>\n",
        ),
    ];
    let tree = files
        .into_iter()
        .map(|(p, c)| (std::path::PathBuf::from(p), c.as_bytes().to_vec()))
        .collect();
    roundhouse::ingest::ingest_app_from_tree(tree).expect("ingest tree")
}

fn show_action_body() -> String {
    let app = app();
    let lcs = roundhouse::lower::lower_controllers_with_arel_views_and_assocs(
        &app.controllers,
        Vec::new(),
        Some(&app.schema),
        &app.views,
        &[],
    );
    let lc = lcs
        .iter()
        .find(|lc| lc.name.0.as_str() == "Rooms::RefreshesController")
        .expect("the controller lowered");
    let m = lc
        .methods
        .iter()
        .find(|m| m.name.as_str() == "show")
        .expect("its show action");
    format!("{:?}", m.body)
}

#[test]
fn a_namespaced_controllers_turbo_stream_template_is_dispatched() {
    let body = show_action_body();
    assert!(
        body.contains("show_turbo_stream"),
        "the format-qualified view must be reachable: {body}"
    );
    assert!(
        body.contains("text/vnd.turbo-stream.html"),
        "with the Content-Type Turbo requires: {body}"
    );
}
