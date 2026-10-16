//! An explicit `params:` local on a jbuilder partial must stay the
//! `params` parameter. When the request-params ivar is renamed to
//! `__rh_ivar_params` to avoid clashing, bare `params` reads must still
//! resolve to the caller's local — not remain a method call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

#[test]
fn explicit_params_local_wins_over_request_params_ivar() {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"widgets/:id\", to: \"widgets#show\", defaults: { format: :json }\nend\n",
        ),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    @payload = { q: \"z\" }\n    render :show\n  end\nend\n",
        ),
        (
            "app/views/widgets/show.json.jbuilder",
            "json.partial! partial: \"widgets/widget\", widget: @widget, params: @payload\n",
        ),
        (
            "app/views/widgets/_widget.json.jbuilder",
            "json.name widget.name\njson.q params[:q]\n",
        ),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    let emitted = target_files(&app, Path::new("fixtures/tiny-blog"), BuildTarget::Ruby).expect("emit");
    let partial = emitted
        .iter()
        .find(|(p, _)| p.ends_with("widgets/_widget_json.rb"))
        .map(|(_, c)| c.as_str())
        .expect("partial");
    let def_line = partial
        .lines()
        .find(|l| l.contains("def self.widget_json"))
        .unwrap_or(partial);
    let sig = def_line.split_once('(').map(|(_, r)| r).unwrap_or(def_line);
    let has_params_token = sig
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|tok| tok == "params");
    assert!(
        has_params_token,
        "explicit params: local must remain a parameter named params:\n{partial}"
    );
    assert!(
        partial.contains("params[\"q\"]") || partial.contains("params[:q]"),
        "bare params read must use the local:\n{partial}"
    );
    assert!(
        !partial.contains("__rh_ivar_params[\"q\"]") && !partial.contains("__rh_ivar_params[:q]"),
        "must not read the renamed request-params ivar for bare params:\n{partial}"
    );
}
