//! Regression: a `helper_method` reachable from a jbuilder `json.cache!`
//! key (or block) that caches the WHOLE template.
//!
//! `json.cache! key do … end` is Jbuilder's own fragment cache; Rails
//! mixes every `helper_method`-marked controller method into the view
//! context, so `current_account` (or any other `helper_method`) is
//! reachable from anywhere in the template, cache! key included.
//! `lower::jbuilder_to_library::walk_cached_template` (roundhouse
//! 981ce8e30, "Campfire fragment rendering") lowers a whole-template
//! `cache!` into an `if ActionView::ViewHelpers.jbuilder_caching? …`
//! wrapper whose key expression is cloned into a brand new
//! `ActionView::ViewHelpers.jbuilder_cache_key(…)` call. The view itself
//! lowers to a `Views::<X>.<template>_json` MODULE FUNCTION with no
//! controller instance, so a bare helper-method call only resolves when
//! `emit::ruby::apply_helper_lowering`'s `rewrite_helper_calls` rewrites
//! it into the per-dispatch `ActionController::Current.controller.<m>`
//! seam. Generic Widget fixture; no app code.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn a_helper_method_in_the_cache_key_is_reachable() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"widgets/:id\", to: \"widgets#show\", defaults: { format: :json }\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\n  helper_method :current_account\n\n  def current_account\n    \"acme\"\n  end\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    render :show\n  end\nend\n",
        )
        .write(
            "app/views/widgets/show.json.jbuilder",
            "json.cache!([current_account, @widget]) do\n  json.id @widget.id\n  json.account current_account\nend\n",
        )
        .run_ruby(
            "widget = Widget.create!(name: \"gizmo\")\nputs Views::Widgets.show_json(widget)\n",
        );
    run.assert_passes();
    assert!(run.stdout.contains("\"acme\""), "stdout: {}\nstderr: {}", run.stdout, run.stderr);
}
