//! Safe navigation in a chain that a lowering rewrites.
//!
//! `a&.b` is bound to a temp (`(__safe_nav1 = a) && __safe_nav1.b`) when
//! `a` is not a plain read, and the lowerings that rewrite
//! `errors.full_messages`, `association(:x).loaded?` and a has_many
//! reader followed by a query method each match the chain written with
//! `.` only. Written with `&.`, as Rails code routinely does where the
//! record, the association or the owner can be nil, the chain was
//! emitted verbatim and raised NoMethodError where Rails answers.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
fn safe_navigation_keeps_the_rewritten_chains() {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end

  create_table "gadgets", force: :cascade do |t|
    t.integer "widget_id", null: false
    t.string "label"
    t.boolean "active", default: true
  end
end
"#,
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  has_many :gadgets\n  validates :name, presence: true\nend\n",
        )
        .write(
            "app/models/gadget.rb",
            r#"class Gadget < ApplicationRecord
  belongs_to :widget

  def owner_loaded_plain?
    widget.association(:gadgets).loaded?
  end

  def owner_loaded_safe?
    widget&.association(:gadgets)&.loaded?
  end
end
"#,
        )
        .write(
            "app/controllers/application_controller.rb",
            r#"class ApplicationController < ActionController::API
  rescue_from ActiveRecord::RecordInvalid, with: :unprocessable

  private

  def unprocessable(e)
    message = e.record&.errors&.full_messages&.first || e.message
    render json: { error: message }, status: :unprocessable_entity
  end
end
"#,
        )
        .write(
            "app/controllers/widgets_controller.rb",
            r#"class WidgetsController < ApplicationController
  def invalid
    Widget.create!(name: nil)
    head :ok
  end

  def loaded
    gadget = Gadget.find(params[:id])
    render json: { plain: gadget.owner_loaded_plain?, safe: gadget.owner_loaded_safe? }
  end

  def labels
    labels = Widget.find_by(name: params[:name])&.gadgets&.where(active: true)&.pluck(:label)
    render json: { labels: labels }
  end
end
"#,
        )
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  post "widgets/invalid", to: "widgets#invalid"
  get "widgets/:id/loaded", to: "widgets#loaded"
  get "widgets/labels", to: "widgets#labels"
end
"#,
        )
        .run_ruby(
            r##"
def call(verb, path, query = "")
  env = { "REQUEST_METHOD" => verb, "PATH_INFO" => path, "QUERY_STRING" => query, "HTTP_ACCEPT" => "application/json", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => "0", "rack.input" => StringIO.new("") }
  status, _headers, body = Main.run_rack(env)
  text = +""
  body.each { |part| text << part }
  [status, text]
end
w = Widget.create!(name: "alpha")
g = Gadget.create!(widget_id: w.id, label: "g1")
Gadget.create!(widget_id: w.id, label: "g2", active: false)
got = [
  call("POST", "/widgets/invalid"),
  call("GET", "/widgets/#{g.id}/loaded"),
  call("GET", "/widgets/labels", "name=alpha"),
  call("GET", "/widgets/labels", "name=none"),
]
want = [
  [422, %({"error":"Name can't be blank"})],
  [200, %({"plain":false,"safe":false})],
  [200, %({"labels":["g1"]})],
  [200, %({"labels":null})],
]
raise got.inspect unless got == want
puts "ALL OK"
"##,
        )
        .assert_passes();
}
