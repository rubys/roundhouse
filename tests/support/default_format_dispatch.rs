//! The app behind the default-format dispatch contract (rubys/roundhouse#1237):
//! one route with `defaults: { format: :json }`, one inside a scope with the
//! same default, one with none. Each action answers from `respond_to` and
//! echoes `params[:format].to_s`. The expected answers are Rails 8.1.4's,
//! from the same routes drawn into a `RouteSet` and called with
//! `Rack::MockRequest` (Rails keeps a `defaults: { format: :json }` Symbol
//! as a Symbol, hence the `to_s`).
//!
//! | path          | with a default | scoped default | no default   |
//! | no extension  | json, "json"   | json, "json"   | html, ""     |
//! | `.json`       | json, "json"   | json, "json"   | json, "json" |
//! | `.html`       | html, "html"   | html, "html"   | html, "html" |

use super::emit_and_run;

pub const CASES: [(&str, &str); 9] = [
    ("/widgets/1", "json:json"),
    ("/widgets/1.json", "json:json"),
    ("/widgets/1.html", "html:html"),
    ("/plain/1", "html:"),
    ("/plain/1.json", "json:json"),
    ("/plain/1.html", "html:html"),
    ("/scoped/1", "json:json"),
    ("/scoped/1.json", "json:json"),
    ("/scoped/1.html", "html:html"),
];

pub fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r##"class WidgetsController < ApplicationController
  def show
    respond_to do |format|
      format.json { render plain: "json:#{params[:format].to_s}" }
      format.html { render plain: "html:#{params[:format].to_s}" }
    end
  end
end
"##,
        )
        .write(
            "config/routes.rb",
            r##"Rails.application.routes.draw do
  get "/widgets/:id", to: "widgets#show", defaults: { format: :json }
  get "/plain/:id", to: "widgets#show"
  scope defaults: { format: :json } do
    get "/scoped/:id", to: "widgets#show"
  end
end
"##,
        )
}
