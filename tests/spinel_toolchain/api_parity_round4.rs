//! The Spinel side of the Rails API parity round 4 fixes this file's
//! siblings test on CRuby, in one app and one `spin build`:
//!
//! * an enum label mapped through the relation's own model
//!   (`tests/enum_model_column_mapping.rs`);
//! * `permit(...).reverse_merge(...)` in an action
//!   (`tests/params_reverse_merge_in_action.rs`);
//! * `request.filtered_parameters` (`tests/request_filtered_parameters.rs`),
//!   whose params and path parameters the server's dispatcher fills;
//! * `json.merge!` of a record's attributes (`tests/jbuilder_merge_attributes.rs`);
//! * safe navigation through rewritten chains (`tests/safe_nav_rewritten_chains.rs`).
//!
//! The dispatcher is the server's (`runtime/spinel/scaffold/main.rb`),
//! so it is a full `spin build`, not a `SPINEL_TESTS` entry.

use super::{emit_and_run, native_http};
use roundhouse::project::BuildTarget;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("config/application.rb", r##"require_relative "boot"
require "rails/all"

module Shop
  class Application < Rails::Application
    config.load_defaults 8.1
    config.api_only = true
  end
end
"##)
        .write("config/initializers/filter_parameter_logging.rb", "Rails.application.config.filter_parameters += [:passw, :name]\n")
        .write("app/controllers/application_controller.rb", r##"class ApplicationController < ActionController::API
  rescue_from ActiveRecord::RecordInvalid, with: :unprocessable

  private

  def unprocessable(e)
    message = e.record&.errors&.full_messages&.first || e.message
    render json: { error: message }, status: :unprocessable_entity
  end
end
"##)
        .write("app/controllers/widgets_controller.rb", r##"class WidgetsController < ApplicationController
  ROLES = %i[admin owner].freeze

  def seed
    alpha = Widget.create!(name: "alpha", role: :admin, price: 3)
    Widget.create!(name: "beta", role: :member, price: 9)
    Widget.create!(name: "gamma", role: :owner, price: 1)
    Gadget.create!(widget_id: alpha.id, label: "g1", role: :guest)
    Gadget.create!(widget_id: alpha.id, label: "g2", role: :owner)
    render plain: Widget.count.to_s
  end

  def admins
    render json: {
      listed: Widget.where(role: ROLES).order(:name).pluck(:name),
      others: Widget.where(price: 0..).where.not(role: :admin).order(:name).pluck(:name),
      gadgets: Gadget.order(:label).where(role: :owner).pluck(:label)
    }
  end

  def staged
    attrs = params.require(:widget).permit(:name, :price).reverse_merge(price: 7, role: :owner)
    widget = Widget.create!(attrs)
    render json: { name: widget.name, price: widget.price, role: widget.role }
  end

  def filtered
    render json: { logged: request.filtered_parameters }
  end

  def invalid
    Widget.create!(name: nil)
    head :ok
  end

  def labels
    labels = Widget.find_by(name: params[:name])&.gadgets&.where(role: :owner)&.pluck(:label)
    render json: { labels: labels, loaded: Gadget.first.owner_loaded? }
  end

  def show
    @widget = Widget.find(params[:id])
  end
end
"##)
        .write("app/models/application_record.rb", r##"class ApplicationRecord < ActiveRecord::Base
  primary_abstract_class
end
"##)
        .write("app/models/widget.rb", r##"class Widget < ApplicationRecord
  enum :role, %i[member admin owner]
  has_many :gadgets
  validates :name, presence: true
end
"##)
        .write("app/models/gadget.rb", r##"class Gadget < ApplicationRecord
  enum :role, %i[guest owner]
  belongs_to :widget

  def owner_loaded?
    widget&.association(:gadgets)&.loaded?
  end
end
"##)
        .write("app/views/widgets/show.json.jbuilder", "json.merge!(@widget.attributes.except(\"id\"))\njson.done true\n")
        .write("config/routes.rb", r##"Rails.application.routes.draw do
  get "/widgets/seed", to: "widgets#seed"
  get "/widgets/admins", to: "widgets#admins"
  post "/widgets/staged", to: "widgets#staged"
  post "/widgets/filtered", to: "widgets#filtered"
  post "/widgets/invalid", to: "widgets#invalid"
  get "/widgets/labels", to: "widgets#labels"
  resources :widgets, only: %i[show]
end
"##)
        .write("db/schema.rb", r##"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "role", default: 0, null: false
    t.integer "price", default: 0
  end

  create_table "gadgets", force: :cascade do |t|
    t.integer "widget_id", null: false
    t.string "label"
    t.integer "role", default: 0, null: false
  end
end
"##)
}

#[test]
#[ignore = "requires Spinel; run in its CI lane"]
fn api_parity_round4_fixes_run_on_spinel() {
    let (tree, errors) = app().emit(BuildTarget::Spinel);
    assert!(errors.is_empty(), "analysis/emission errors:\n{}", errors.join("\n"));

    native_http::build(&tree);
    let server = native_http::Server::start(&tree);
    let json = "application/json";

    let seed = server.get("/widgets/seed");
    assert_eq!((seed.status, seed.body.as_str()), (200, "3"), "seed:\n{}", server.log());

    let admins = server.get("/widgets/admins");
    assert_eq!(
        admins.body,
        r#"{"listed":["alpha","gamma"],"others":["beta","gamma"],"gadgets":["g2"]}"#,
        "enum by model:\n{}",
        server.log()
    );

    let staged = server.post("/widgets/staged", json, r#"{"widget":{"name":"delta"}}"#);
    assert_eq!(
        (staged.status, staged.body.as_str()),
        (200, r#"{"name":"delta","price":7,"role":"owner"}"#),
        "reverse_merge:\n{}",
        server.log()
    );

    let filtered = server.post("/widgets/filtered", json, r#"{"widget":{"name":"delta","password":"x","size":"3"}}"#);
    assert_eq!(
        filtered.body,
        r#"{"logged":{"widget":{"name":"[FILTERED]","password":"[FILTERED]","size":"3"},"controller":"widgets","action":"filtered"}}"#,
        "filtered_parameters:\n{}",
        server.log()
    );

    let invalid = server.post("/widgets/invalid", json, "{}");
    assert_eq!(
        (invalid.status, invalid.body.as_str()),
        (422, r#"{"error":"Name can't be blank"}"#),
        "errors&.full_messages:\n{}",
        server.log()
    );

    let labels = server.get("/widgets/labels?name=alpha");
    assert_eq!(labels.body, r#"{"labels":["g2"],"loaded":false}"#, "safe-nav chains:\n{}", server.log());
    let none = server.get("/widgets/labels?name=none");
    assert_eq!(none.body, r#"{"labels":null,"loaded":false}"#, "safe-nav nil:\n{}", server.log());

    let show = server.get("/widgets/1.json");
    assert_eq!(
        show.body,
        r#"{"name":"alpha","role":"admin","price":3,"done":true}"#,
        "attributes merge:\n{}",
        server.log()
    );
}
