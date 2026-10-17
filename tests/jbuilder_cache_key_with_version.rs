//! Regression for the jbuilder `cache!` key evaluating
//! `ActiveRecord::Relation#cache_key_with_version`.
//!
//! Jbuilder's `cache!(key=nil, options={})` takes `key` as a plain
//! positional parameter, so Ruby evaluates the key expression as a
//! method-call argument BEFORE `cache!` ever checks
//! `@context.controller.perform_caching` — the key is built whether or
//! not caching is on (confirmed against jbuilder 2.15.1's
//! `jbuilder_template.rb`). Real Rails' `ActiveRecord::Relation` answers
//! `cache_key_with_version` (`active_record/relation.rb`,
//! `"<table>/query-<digest>-<count>-<timestamp>"`), so
//! `json.cache!([Tenant.current.id, @widgets.cache_key_with_version], …)`
//! renders under Rails with caching on OR off.
//!
//! roundhouse's own `ActiveRecord::Relation` runtime
//! (`runtime/ruby/active_record/relation.rb`) never defined
//! `cache_key_with_version`, so the same view raised
//! `undefined method 'cache_key_with_version' for an instance of
//! ActiveRecord::Relation` in the emitted app — reproduced here with a
//! generic Widget fixture (no app code).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_collection_cache_key_renders_with_caching_off() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.timestamps\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"widgets\", to: \"widgets#index\", defaults: { format: :json }\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.all\n    render :index\n  end\nend\n",
        )
        .write(
            "app/views/widgets/index.json.jbuilder",
            "json.cache!([@widgets.cache_key_with_version], expires_in: 5.minutes) do\n  json.widgets(@widgets) do |widget|\n    json.id widget.id\n    json.name widget.name\n  end\nend\n",
        )
        .run_ruby(
            "Widget.create!(name: \"gizmo\")\nputs Views::Widgets.index_json(Widget.all)\n",
        );
    run.assert_passes();
    assert!(run.stdout.contains("\"gizmo\""), "stdout: {}\nstderr: {}", run.stdout, run.stderr);
}
