//! A path helper given a nil or empty required segment raises
//! `ActionController::UrlGenerationError` with Rails' message, as
//! actionpack 8.1.4 does, instead of building "/articles//comments"
//! (roundhouse#1475). Its oracle, on the same calls:
//!
//!     ruby tests/route_helper_nil_segment/oracle.rb
//!
//! The oracle calls `article_comments_path(nil, format: :json)` where
//! the app calls the generated `article_comments_json_path(nil)`. Rails'
//! message there is its "possible unmatched constraints" form, which
//! isn't modelled, so that check asks only that it raises.

#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

const SCRIPT: &str = r##"
def check(label, got, want)
  raise "#{label}: #{got.inspect} (want #{want.inspect})" unless got == want
end
def error_of
  yield
  "no error"
rescue ActionController::UrlGenerationError => e
  e.message
end
check("a nil segment", error_of { RouteHelpers.article_comments_path(nil) },
  'No route matches {action: "create", article_id: nil, controller: "comments"}, missing required keys: [:article_id]')
check("an empty segment", error_of { RouteHelpers.article_comments_path("") },
  'No route matches {action: "create", article_id: "", controller: "comments"}, missing required keys: [:article_id]')
check("one of two segments", error_of { RouteHelpers.article_comment_path(nil, 5) },
  'No route matches {action: "destroy", article_id: nil, controller: "comments", id: "5"}, missing required keys: [:article_id]')
check("both segments", error_of { RouteHelpers.article_comment_path(nil, nil) },
  'No route matches {action: "destroy", article_id: nil, controller: "comments", id: nil}, missing required keys: [:article_id, :id]')
check("missing keys, sorted", error_of { RouteHelpers.zone_item_path(nil, nil) },
  'No route matches {action: "show", controller: "comments", id: nil, zone_id: nil}, missing required keys: [:id, :zone_id]')
check("a format variant raises", error_of { RouteHelpers.article_comments_json_path(nil) }.start_with?("No route matches {"), true)
check("a filled segment", RouteHelpers.article_comments_path(1), "/articles/1/comments")
puts "nil segment passed"
"##;

/// real-blog, plus a route whose segments aren't in sorted order and a
/// view call that generates the `.json` variant of `article_comments_path`.
fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .edit(
            "config/routes.rb",
            "  root \"articles#index\"\n",
            "  root \"articles#index\"\n  get \"/zones/:zone_id/items/:id\", to: \"comments#show\", as: :zone_item\n",
        )
        .edit(
            "app/views/articles/show.html.erb",
            "py-2 rounded\" %>\n<% end %>\n",
            "py-2 rounded\" %>\n<% end %>\n<%= link_to \"JSON\", article_comments_path(@article, format: :json) %>\n",
        )
}

#[test]
fn a_nil_required_segment_raises_on_ruby() {
    let run = app().run_ruby(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nil segment passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_nil_required_segment_raises_on_spinel() {
    let run = app().run_spinel_with_db(SCRIPT);
    run.assert_passes();
    assert!(run.stdout.contains("nil segment passed"), "{}", run.stdout);
}
