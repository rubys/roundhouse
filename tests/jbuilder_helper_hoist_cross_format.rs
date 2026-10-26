//! Regression found by CodeRabbit on roundhouse PR #1034, companion to
//! `tests/jbuilder_helper_hoist_cross_view.rs` (which covers two SIBLING
//! jbuilder views): the same hoisted-constant collision also happens
//! across FORMATS, not just within one.
//!
//! `emit_lowered_views` (html/ERB) and `emit_lowered_jbuilder_views`
//! (json/jbuilder) each lower their own views into a SEPARATE slice of
//! `LibraryClass`es and call `library::apply_helper_lowering` on that
//! slice independently. Its tail, `hoist_pure_helper_constants`, dedupes
//! a hoisted helper constant's name against a `used` set built fresh
//! FOR THE SLICE IT IS GIVEN — so an html view and a jbuilder view that
//! reopen the SAME `Views::<Plural>` module (`index.html.erb` and
//! `show.json.jbuilder`, both under `app/views/widgets/`) and each call
//! the same hoistable helper (`image_tag`) with the same first string
//! literal but a DIFFERENT remaining option (`alt:`) mint the identical
//! `HOISTED_IMAGE_TAG_BOOST_SVG` constant in two separate files. At Ruby
//! load time the second file's top-level assignment silently overwrites
//! the first's, so whichever view's file loads first renders the OTHER
//! view's alt text.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn an_html_and_a_jbuilder_view_do_not_cross_contaminate_hoisted_constants() {
    let run = emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"widgets\", to: \"widgets#index\", defaults: { format: :html }\n  get \"widgets/:id\", to: \"widgets#show\", defaults: { format: :json }\nend\n",
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
            "app/views/widgets/index.html.erb",
            "<%= image_tag(\"boost.svg\", alt: \"Index Icon\") %>\n",
        )
        .write(
            "app/views/widgets/show.json.jbuilder",
            "json.icon image_tag(\"boost.svg\", alt: \"Show Icon\")\n",
        )
        .run_ruby(
            "Widget.create!(name: \"gizmo\")\nputs Views::Widgets.index\nputs Views::Widgets.show_json\n",
        );
    run.assert_passes();
    assert!(
        run.stdout.contains("Index Icon"),
        "index.html.erb's own alt text must survive: stdout: {}\nstderr: {}",
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
