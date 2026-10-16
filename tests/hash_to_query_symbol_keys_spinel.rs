//! `Hash#to_query` on a Symbol-keyed Hash whose values are not all
//! scalars — a value read from `params`, or a nested Hash — lowers to
//! `ActiveSupport.to_query`. Rails' `Hash#to_query` takes any key
//! (`to_s` each, the encoded pairs sorted), so a literal
//! `{ page: …, q: … }.to_query` must compile and render the same bytes
//! as a String-keyed one. The native run needs a Spinel compiler, so it
//! is `#[ignore]`d like its sibling suites; CI's `spinel-framework` job
//! runs it with `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/search\", to: \"searches#show\"\n  get \"/filter\", to: \"searches#filter\"\n  get \"/keyed\", to: \"searches#keyed\"\nend\n")
        .write("app/controllers/searches_controller.rb", r#"class SearchesController < ApplicationController
  def show
    render plain: { q: params[:q], page: params[:page], sort: "name" }.to_query
  end

  def filter
    render plain: { filter: { status: "open", tags: ["a b", "c"] }, page: 2, empty: {} }.to_query
  end

  def keyed
    render plain: { "sort" => params[:sort], "b" => "1" }.to_query
  end
end
"#)
}

/// The expected bodies are activesupport 8.1.4's for the same Hashes
/// (`ruby -ractive_support/all`): keys stringified, the encoded pairs
/// sorted, a nested Hash bracketed and its own level sorted, an Array
/// value as repeated `key[]=`, an empty Hash value dropped, a space as `+`.
/// The String-keyed `keyed` is the shape the signature was written for
/// and must keep compiling.
const SCRIPT: &str = r#"
require_relative "app/controllers/searches_controller"
controller = SearchesController.new
controller.params = {"q" => "red shoes", "page" => "3"}
controller.process_action(:show)
raise controller.body unless controller.body == "page=3&q=red+shoes&sort=name"
controller = SearchesController.new
controller.process_action(:filter)
raise controller.body unless controller.body == "filter%5Bstatus%5D=open&filter%5Btags%5D%5B%5D=a+b&filter%5Btags%5D%5B%5D=c&page=2"
controller = SearchesController.new
controller.params = {"sort" => "top"}
controller.process_action(:keyed)
raise controller.body unless controller.body == "b=1&sort=top"
puts "symbol-keyed to_query passed"
"#;

#[test]
fn symbol_keyed_to_query_renders_as_rails_on_cruby() {
    app().run_ruby(SCRIPT).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn symbol_keyed_to_query_compiles_and_renders_as_rails_on_spinel() {
    app().run_spinel_with_rbs(SCRIPT).assert_passes();
}
