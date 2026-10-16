//! CodeRabbit 4277530506 on rubys/roundhouse#953
//! (`src/lower/controller_to_library/rewrites.rs:513`).
//!
//! `render content_type: :css` (or `"text/css"`) names no template —
//! `resolve_formats_only_render` binds it to the CURRENT action's own
//! template, and `rewrite_render_to_views`'s `is_css` arm looked up
//! only the format-qualified stem (`show_css`). An action whose only
//! template is a format-agnostic `show.erb` (no `.css.erb` sibling at
//! all) has no `show_css` contract, so the render raised
//! `ActionView::MissingTemplate` for a template that was right there —
//! Rails itself renders a format-agnostic template for ANY requested
//! format, the same answer `feed_fmt`'s `rss`/`atom`/`xml` fallback
//! already gives a few lines above this one. `css_qualified` gates the
//! qualified lookup the same way `feed_qualified` does, falling back
//! to the bare stem while still tagging the response `text/css`.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const APPLICATION_CONTROLLER: &str = "\
class ApplicationController < ActionController::Base
end
";

const GADGET_STYLES_CONTROLLER: &str = "\
class GadgetStylesController < ApplicationController
  def show
    @color = \"teal\"
    render content_type: \"text/css\"
  end
end
";

const ROUTES: &str = "\
Rails.application.routes.draw do
  get 'gadget.css', to: 'gadget_styles#show'
end
";

/// Only `show.erb` exists — no `show.css.erb`, no `show.html.erb`
/// either. A format-agnostic template answers any requested format.
#[test]
fn content_type_css_with_only_a_format_agnostic_template_renders_it_as_css() {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", APPLICATION_CONTROLLER)
        .write("app/controllers/gadget_styles_controller.rb", GADGET_STYLES_CONTROLLER)
        .write(
            "app/views/gadget_styles/show.erb",
            ".gadget { color: <%= @color %>; }\n",
        )
        .write("config/routes.rb", ROUTES)
        .run_ruby(
            r#"
status, headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/gadget.css", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
raise "status #{status}" unless status == 200
raise "Content-Type #{headers["content-type"].inspect}" unless headers["content-type"] == "text/css"
raise "body #{body.join.inspect}" unless body.join.strip == ".gadget { color: teal; }"
puts "ALL OK"
"#,
        )
        .assert_passes();
}
