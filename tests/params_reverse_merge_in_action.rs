//! `<permit-chain>.reverse_merge(defaults)` anywhere in an action, not
//! only as a params helper's return value.
//!
//! Ingest spells `hash.reverse_merge(defaults)` as
//! `defaults.merge(hash)`. The params lowering recognized that spelling
//! only at a method body's tail; anywhere else the permit chain inside
//! it was rewritten to the typed `WidgetParams` on its own, leaving
//! `{ price: 7 }.merge(WidgetParams.from_raw(...))`: a `TypeError` at
//! request time where Rails answers the permitted attributes with the
//! defaults filling the keys the request left out.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "price", default: 0
    t.integer "role", default: 0, null: false
  end
end
"#;

fn app(controller: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  enum :role, %i[member admin owner]\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/widgets_controller.rb", controller)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: %i[create]\nend\n",
        )
}

const PROBE: &str = r##"
def post(body)
  env = { "REQUEST_METHOD" => "POST", "PATH_INFO" => "/widgets", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => body.bytesize.to_s, "rack.input" => StringIO.new(body) }
  status, _headers, out = Main.run_rack(env)
  text = +""
  out.each { |part| text << part }
  [status, text]
end
got = [post(%({"widget":{"name":"delta"}})), post(%({"widget":{"name":"eps","price":"4"}}))]
want = [[200, %({"name":"delta","price":7,"role":"owner"})], [200, %({"name":"eps","price":4,"role":"owner"})]]
raise got.inspect unless got == want
puts "ALL OK"
"##;

#[test]
fn reverse_merge_assigned_in_an_action_fills_the_missing_keys() {
    app(r#"class WidgetsController < ApplicationController
  def create
    attrs = params.require(:widget).permit(:name, :price).reverse_merge(price: 7, role: :owner)
    widget = Widget.create!(attrs)
    render json: { name: widget.name, price: widget.price, role: widget.role }
  end
end
"#)
    .run_ruby(PROBE)
    .assert_passes();
}

#[test]
fn reverse_merge_passed_straight_to_create() {
    app(r#"class WidgetsController < ApplicationController
  def create
    widget = Widget.create!(params.require(:widget).permit(:name, :price).reverse_merge({ price: 7, role: :owner }))
    render json: { name: widget.name, price: widget.price, role: widget.role }
  end
end
"#)
    .run_ruby(PROBE)
    .assert_passes();
}
