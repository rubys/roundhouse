//! `.css.erb` view templates — rubys/roundhouse ranked-gap #17.
//!
//! `walk_erb`'s format allow-list (`ingest/app.rs`) skipped any
//! non-html ERB format it didn't recognize. `css` joins the formats
//! that already ride it (`svg`/`json`/`js`/`pdf`/`csv`/`txt`), lowered
//! the same way: `<action>_css`, beside an html sibling when one
//! exists (none does here — `GadgetStylesController#show` serves CSS
//! only, Mastodon's `custom_css#show` shape).
//!
//! `render content_type: "text/css"` names no template at all — Rails
//! resolves it to the CURRENT action's own template, in this format.
//! `resolve_formats_only_render` already does exactly this for
//! `formats:`/`layout:`; `content_type:` joins them.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::collections::HashMap;
use std::path::PathBuf;

use roundhouse::ingest::ingest_app_from_tree;

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

fn gadget_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", APPLICATION_CONTROLLER)
        .write("app/controllers/gadget_styles_controller.rb", GADGET_STYLES_CONTROLLER)
        .write(
            "app/views/gadget_styles/show.css.erb",
            ".gadget { color: <%= @color %>; }\n",
        )
        .write("config/routes.rb", ROUTES)
}

/// `GadgetStylesController#show` renders `show.css.erb` with
/// `Content-Type: text/css`, on real CRuby via the emitted app's own
/// Rack entry point.
#[test]
fn css_erb_view_renders_with_text_css_content_type() {
    gadget_app()
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

/// A format `walk_erb`'s allow-list still skips (`.ics.erb` — an
/// iCalendar feed, not modeled) must abort STRICT ingest instead of
/// silently dropping the template and emitting the controller with no
/// view and no error. `.ics` is picked because it rides the exact
/// branch `css` just left (`record_skipped_view`'s "non-html format"
/// arm in `walk_erb`) — any format still outside the allow-list
/// exercises it the same way.
#[test]
fn a_still_unsupported_erb_format_fails_strict_ingest_instead_of_dropping_silently() {
    let files: Vec<(&str, &str)> = vec![
        ("db/schema.rb", "ActiveRecord::Schema[7.1].define(version: 1) do\nend\n"),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        (
            "app/controllers/gadget_events_controller.rb",
            "class GadgetEventsController < ApplicationController\n  def show\n  end\nend\n",
        ),
        (
            "app/views/gadget_events/show.ics.erb",
            "BEGIN:VCALENDAR\nEND:VCALENDAR\n",
        ),
    ];
    let tree: HashMap<PathBuf, Vec<u8>> =
        files.into_iter().map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec())).collect();
    let err = ingest_app_from_tree(tree)
        .expect_err("strict ingest must abort on a still-unsupported ERB format, not drop it");
    assert!(
        err.to_string().contains("view template not ingested: erb"),
        "expected the ics-format view to be named as the ingest gap, got: {err}"
    );
}
