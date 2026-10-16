//! An action defined in a controller concern is dispatched (#859).
//!
//! Rails' actions are a controller's public methods, wherever they were
//! written: a public method a concern defines is an action of every
//! controller that includes it, runs the filters that name it, and
//! renders the includer's template. The ingest spliced every concern
//! method behind the includer's private marker, as a helper, so
//! `process_action` had no arm for it and the route answered an empty
//! 200. A routed public concern method now joins the includer's public
//! actions; an unrouted one, and a private one, stays a helper.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/concerns/gadget_actions.rb",
            r##"module GadgetActions
  extend ActiveSupport::Concern

  included do
    before_action :load_note, only: %i[show counts]
  end

  def show
    @note = "#{@note} from concern"
  end

  def counts
    render json: { counts: [1, 2], note: @note, label: label_for(3) }
  end

  def label_for(n)
    "n#{n}"
  end

  private

  def load_note
    @note = "loaded"
  end
end
"##,
        )
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  include GadgetActions\n\n  def index\n    render json: { own: true }\n  end\nend\n",
        )
        .write(
            "app/controllers/blobs_controller.rb",
            "class BlobsController < ApplicationController\n  include GadgetActions\nend\n",
        )
        .write("app/views/widgets/show.json.jbuilder", "json.note @note\n")
        .write("app/views/blobs/show.json.jbuilder", "json.blob @note\n")
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: %i[index show] do\n    get :counts, on: :collection\n  end\n  resources :blobs, only: %i[show]\nend\n",
        )
}

#[test]
fn concern_actions_are_dispatched_and_rendered() {
    app()
        .run_ruby(
            r##"
def probe(path)
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, text]
end
{
  "/widgets.json" => [200, %({"own":true})],
  "/widgets/1.json" => [200, %({"note":"loaded from concern"})],
  "/widgets/counts.json" => [200, %({"counts":[1,2],"note":"loaded","label":"n3"})],
  "/blobs/1.json" => [200, %({"blob":"loaded from concern"})],
}.each do |path, want|
  got = probe(path)
  raise "#{path} #{got.inspect}" unless got == want
end
puts "ALL OK"
"##,
        )
        .assert_passes();
}
