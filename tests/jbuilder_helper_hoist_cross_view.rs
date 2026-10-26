//! Regression found by CodeRabbit on roundhouse PR #1034:
//! `emit_lowered_jbuilder_views` ran `apply_helper_lowering` (and its
//! tail, `hoist_pure_helper_constants`) once PER VIEW, on that view's
//! own one-method slice. `hoist_pure_helper_constants`'s dedupe name
//! (`hoisted_const_name`) is derived from the helper method plus its
//! first String literal argument only; the uniqueness-tracking `used`
//! set it builds is local to the slice it is given. Two SIBLING
//! jbuilder views reopening the same `Views::<Plural>` module
//! (`index.json.jbuilder` and `show.json.jbuilder` both under
//! `app/views/widgets/`) that call the same hoistable helper
//! (`image_tag`) with the same first string literal but DIFFERENT
//! remaining options mint the identical constant name in two separate
//! calls — at Ruby load time the second file's top-level assignment
//! silently overwrites the first's, so whichever view's file loads
//! first ends up rendering the OTHER view's value.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn sibling_jbuilder_views_do_not_cross_contaminate_hoisted_constants() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"widgets\", to: \"widgets#index\", defaults: { format: :json }\n  get \"widgets/:id\", to: \"widgets#show\", defaults: { format: :json }\nend\n",
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
            "class WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.all\n    render :index\n  end\n\n  def show\n    @widget = Widget.find(params[:id])\n    render :show\n  end\nend\n",
        )
        .write(
            "app/views/widgets/index.json.jbuilder",
            "json.icon image_tag(\"boost.svg\", alt: \"Index Icon\")\n",
        )
        .write(
            "app/views/widgets/show.json.jbuilder",
            "json.icon image_tag(\"boost.svg\", alt: \"Show Icon\")\n",
        )
        .run_ruby(
            "Widget.create!(name: \"gizmo\")\nputs Views::Widgets.index_json\nputs Views::Widgets.show_json\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("Index Icon"),
        "index.json.jbuilder's own alt text must survive: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
    assert!(
        run.stdout.contains("Show Icon"),
        "show.json.jbuilder's own alt text must survive: stdout: {}\nstderr: {}",
        run.stdout,
        run.stderr
    );
}
