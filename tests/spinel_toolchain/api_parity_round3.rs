//! The Spinel side of the Rails API parity fixes this file's siblings
//! test on CRuby, in one app and one `spin build`:
//!
//! * a 404 in a JSON request answers `{"status":404,"error":"Not Found"}`
//!   (`tests/json_not_found.rs`);
//! * `match "/", via: [...]` beside a `root` (`tests/match_root_via_verbs.rs`);
//! * `Relation#all` and `distinct(value)` (`tests/relation_all_distinct_value.rs`);
//! * `rescue_from` reaching `exception.cause` (`tests/rescue_from_cause.rs`);
//! * the health controller's JSON arm (`tests/rails_health_json.rs`);
//! * a jbuilder template whose action sets no ivar (`tests/jbuilder_action_without_ivar.rs`);
//! * `where(enum: CONSTANT)` (`tests/enum_where_constant_list.rs`);
//! * `json.merge!` (`tests/jbuilder_merge.rs`);
//! * an action defined in a concern (`tests/concern_action_dispatch.rs`).
//!
//! The dispatcher is the server's (`runtime/spinel/scaffold/main.rb`),
//! so it is a full `spin build`, not a `SPINEL_TESTS` entry.

use super::{emit_and_run, native_http};
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", r##"class ApplicationController < ActionController::API
  class Missing < StandardError; end
  rescue_from Missing, with: :missing

  private

  def missing(e)
    render json: { error: e.message }, status: :unprocessable_entity
  end
end
"##)
        .write("app/controllers/concerns/gadget_actions.rb", r##"module GadgetActions
  extend ActiveSupport::Concern

  def counts
    render json: { counts: Widget.count }
  end
end
"##)
        .write("app/controllers/errors_controller.rb", r##"class ErrorsController < ApplicationController
  def routing
    render json: { error: "No route matches [#{request.request_method}] #{request.path}" }, status: :not_found
  end
end
"##)
        .write("app/controllers/gizmos_controller.rb", r##"class GizmosController < ApplicationController
  def index; end

  def show; end
end
"##)
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  include GadgetActions

  ROLES = %i[admin owner].freeze

  def seed
    Widget.create!(name: "alpha", role: :admin, price: 3)
    Widget.create!(name: "beta", role: :member, price: 9)
    Widget.create!(name: "beta", role: :owner, price: 1)
    render plain: Widget.count.to_s
  end

  def show
    render json: { name: Widget.find(params[:id]).name }
  end

  def admins
    render json: { names: Widget.where(role: ROLES).order(:name).pluck(:name) }
  end

  def names
    render json: { names: Widget.distinct(:name).pluck(:name).sort }
  end

  def extra
    render json: { count: Widget.where(price: 0..).all.count }
  end

  def wrap
    raise Missing, "widget missing"
  rescue StandardError => e
    raise ArgumentError, "while handling: #{e.message}"
  end
end
"##)
        .write("app/models/application_record.rb", r##"class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
"##)
        .write("app/models/widget.rb", r##"class Widget < ApplicationRecord
  enum :role, %i[member admin owner]
end
"##)
        .write("app/views/gizmos/index.json.jbuilder", "json.ok true\n")
        .write("app/views/gizmos/show.json.jbuilder", "json.merge!({ kind: \"gizmo\", version: 2 })\njson.id 1\n")
        .write("config/routes.rb", r##"Rails.application.routes.draw do
  root to: "rails/health#show"
  get "up" => "rails/health#show"
  resources :gizmos, only: %i[index show]
  get "/widgets/seed", to: "widgets#seed"
  get "/widgets/admins", to: "widgets#admins"
  get "/widgets/names", to: "widgets#names"
  get "/widgets/extra", to: "widgets#extra"
  get "/widgets/counts", to: "widgets#counts"
  post "/widgets/wrap", to: "widgets#wrap"
  resources :widgets, only: %i[show]
  match "/", to: "errors#routing", via: %i[post delete]
end
"##)
        .write("db/schema.rb", r##"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "role", default: 0, null: false
    t.integer "price", default: 0
  end
end
"##)
}

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn api_parity_round3_fixes_run_on_spinel() {
    let (tree, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));

    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let json = [("Accept", "application/json")];

    let seed = server.get("/widgets/seed");
    assert_eq!((seed.status, seed.body.as_str()), (200, "3"), "seed:\n{}", server.log());

    for path in ["/nope", "/nope.json", "/widgets/99"] {
        let res = server.get_with_headers(path, &json);
        assert_eq!((res.status, res.body.as_str()), (404, r#"{"status":404,"error":"Not Found"}"#), "{path}:\n{}", server.log());
        assert_eq!(res.headers.get("content-type").map(String::as_str), Some("application/json; charset=UTF-8"), "{path}");
    }
    let html = server.get("/nope");
    assert!(html.status == 404 && html.body.contains("404 Not Found"), "html 404: {} {}", html.status, html.body);

    for verb in ["POST", "DELETE"] {
        let res = server.request(verb, "/", &[("Content-Type", "application/json")], "{}");
        assert_eq!(
            (res.status, res.body.clone()),
            (404, format!(r#"{{"error":"No route matches [{verb}] /"}}"#)),
            "{verb} /:\n{}",
            server.log()
        );
    }

    let names = server.get("/widgets/names");
    assert_eq!(names.body, r#"{"names":["alpha","beta"]}"#, "distinct:\n{}", server.log());
    let extra = server.get("/widgets/extra");
    assert_eq!(extra.body, r#"{"count":3}"#, "all:\n{}", server.log());

    let wrap = server.request("POST", "/widgets/wrap", &[("Content-Type", "application/json")], "{}");
    assert_eq!((wrap.status, wrap.body.as_str()), (422, r#"{"error":"widget missing"}"#), "cause:\n{}", server.log());

    let up = server.get("/up.json");
    assert!(
        up.status == 200 && up.body.starts_with(r#"{"status":"up","timestamp":""#),
        "health json: {} {}\n{}",
        up.status,
        up.body,
        server.log()
    );

    let index = server.get("/gizmos.json");
    assert_eq!(index.body, r#"{"ok":true}"#, "no-ivar template:\n{}", server.log());

    let admins = server.get("/widgets/admins");
    assert_eq!(admins.body, r#"{"names":["alpha","beta"]}"#, "enum constant:\n{}", server.log());

    let merged = server.get("/gizmos/1.json");
    assert_eq!(merged.body, r#"{"kind":"gizmo","version":2,"id":1}"#, "merge!:\n{}", server.log());

    let counts = server.get("/widgets/counts");
    assert_eq!((counts.status, counts.body.as_str()), (200, r#"{"counts":3}"#), "concern action:\n{}", server.log());
}
