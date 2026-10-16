//! An Integer under a route helper's query option (`widgets_path(page:
//! page + 1)`) reaches the helper as its `to_s`, the `String?` the
//! helper's option is typed.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const CONTROLLER: &str = "class WidgetsController < ApplicationController
  def index
    render json: page_paths(current_page)
  end

  def page_paths(page)
    [widgets_path(page: page + 1), widgets_path(page: 2), widgets_path(page: \"3\")]
  end

  private

  def current_page
    page = params[:page].to_i
    page.positive? ? page : 1
  end
end
";

const ASSERTIONS: &str = r#"
require_relative "app/controllers/widgets_controller"

paths = WidgetsController.new.page_paths(1)
raise paths.inspect unless paths == ["/widgets?page=2", "/widgets?page=2", "/widgets?page=3"]
puts "page paths passed"
"#;

fn example() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/widgets_controller.rb", CONTROLLER)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: :index\nend\n")
}

#[test]
fn an_integer_query_option_is_stringified_at_the_call_site() {
    let (emitted, _) = example().emit(BuildTarget::Ruby);
    let controller = std::fs::read_to_string(emitted.join("app/controllers/widgets_controller.rb")).expect("read the emitted controller");
    assert!(
        controller.contains(r#"[RouteHelpers.widgets_path(page: (page + 1).to_s), RouteHelpers.widgets_path(page: 2.to_s), RouteHelpers.widgets_path(page: "3")]"#),
        "{controller}"
    );
}

#[test]
fn an_integer_query_option_runs_in_ruby() {
    example().run_ruby(ASSERTIONS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn an_integer_query_option_runs_on_spinel_with_rbs() {
    example().run_spinel_with_rbs(ASSERTIONS).assert_passes();
}
