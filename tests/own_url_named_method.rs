//! A bare call to a method the class defines itself wins over a route
//! helper, even when its name reads like one. Rails' url helpers are
//! mixed in beneath the class's own methods, so `widget_from_url` in a
//! class that defines it is that method, not route `widget` with a
//! `.from` format, and a class's own `widgets_url` shadows the route's.
//!
//! Rails 8.1.4 (actionpack 8.1.4, the route set's `url_helpers`
//! included into each class): `WidgetFinder#widget` is `"found acme"`,
//! `#listing` is `"own list"`, and `WidgetsController#links` is
//! `["own", "/widgets/7", "/widgets/7.json"]`, even with another class
//! defining a `widget_json_path` of its own. Before, the finder's two
//! calls emitted as
//! `url_for_path(RouteHelpers.widget_from_path)` (a helper synthesized
//! for a `.from` format, raising ArgumentError on CRuby) and
//! `url_for_path(RouteHelpers.widgets_path)`.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const FINDER: &str = "class WidgetFinder
  def initialize(name)
    @name = name
  end

  def widget
    widget_from_url
  end

  def listing
    widgets_url
  end

  private

  def widget_from_url
    \"found #{@name}\"
  end

  def widgets_url
    \"own list\"
  end
end
";

const CONTROLLER: &str = "class WidgetsController < ApplicationController
  def index
    render plain: links.join(\" \")
  end

  def links
    [widgets_url, widget_path(7), widget_path(7, format: :json)]
  end

  private

  def widgets_url
    \"own\"
  end
end
";

// Defines a method named like the format variant the controller asks
// for; it is this class's method, and the variant is still generated.
const FORMATTER: &str = "class FeedFormatter
  def widget_json_path
    \"own json\"
  end
end
";

const ASSERTIONS: &str = r#"
require_relative "app/models/widget_finder"
require_relative "app/controllers/widgets_controller"

found = WidgetFinder.new("acme").widget
raise found.inspect unless found == "found acme"
listing = WidgetFinder.new("acme").listing
raise listing.inspect unless listing == "own list"
links = WidgetsController.new.links
raise links.inspect unless links == ["own", "/widgets/7", "/widgets/7.json"]
puts "own url methods passed"
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/models/widget_finder.rb", FINDER)
        .write("app/models/feed_formatter.rb", FORMATTER)
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :show]\nend\n")
}

#[test]
fn an_own_url_named_method_is_called_not_routed() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let finder = std::fs::read_to_string(emitted.join("app/models/widget_finder.rb")).expect("read the emitted finder");
    assert!(finder.contains("    widget_from_url\n"), "{finder}");
    assert!(finder.contains("    widgets_url\n"), "{finder}");
    let helpers = std::fs::read_to_string(emitted.join("app/route_helpers.rb")).expect("read the route helpers");
    assert!(!helpers.contains("widget_from_path"), "{helpers}");
    let controller = std::fs::read_to_string(emitted.join("app/controllers/widgets_controller.rb")).expect("read the emitted controller");
    assert!(controller.contains("[self.widgets_url, RouteHelpers.widget_path(7), RouteHelpers.widget_json_path(7)]"), "{controller}");
}

#[test]
fn an_own_url_named_method_runs_in_ruby() {
    example().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_own_url_named_method_runs_on_spinel_with_rbs() {
    example().run_spinel_with_rbs(ASSERTIONS).assert_passes();
}
