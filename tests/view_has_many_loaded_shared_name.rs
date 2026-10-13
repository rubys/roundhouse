//! `record.<has_many>.loaded?` in a view flattens onto the synthesized
//! `<has_many>_loaded?` when more than one model declares that has_many.
//!
//! `lower::assoc_loaded` found a view receiver's model by the
//! association's name alone, and that answered nothing as soon as a
//! second model declared the same `has_many` (`Widget` and `Gadget`
//! both `has_many :parts`). The partial kept `widget.parts.loaded?`,
//! which raised `NoMethodError: undefined method 'loaded?' for an
//! instance of Array`. The `association(:name).loaded?` form already
//! widened to "some model declares the name"; the two-hop form now does
//! the same, and still leaves a receiver typed as a model that lacks
//! the association, or any receiver in a model method, as written.
//!
//! Rails answers `[{"id":1,"name":"alpha","preloaded":true}]`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const SCHEMA: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "gadgets", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "parts", force: :cascade do |t|
    t.string "name", null: false
    t.integer "widget_id"
    t.integer "gadget_id"
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :parts\nend\n")
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  has_many :parts\nend\n")
        .write(
            "app/models/part.rb",
            "class Part < ApplicationRecord\n  belongs_to :widget, optional: true\n  belongs_to :gadget, optional: true\nend\n",
        )
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.includes(:parts)\n  end\nend\n",
        )
        .write(
            "app/views/widgets/index.json.jbuilder",
            "json.array!(@widgets) do |widget|\n  json.partial!(widget, partial: \"widgets/widget\", as: :widget)\nend\n",
        )
        .write(
            "app/views/widgets/_widget.json.jbuilder",
            "json.call(widget, :id, :name)\njson.preloaded(widget.parts.loaded?)\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n")
}

#[test]
fn shared_has_many_name_in_a_view_flattens_onto_the_predicate() {
    let (emitted, _errors) = app().emit(BuildTarget::Spinel);
    let source = std::fs::read_to_string(emitted.join("app/views/widgets/_widget_json.rb"))
        .expect("_widget_json.rb emitted");
    assert!(source.contains("widget.parts_loaded?"), "expected the flat predicate:\n{source}");
    assert!(!source.contains(".parts.loaded?"), "the two-hop call reaches an Array:\n{source}");
}

#[test]
fn shared_has_many_name_in_a_view_renders_rails_answer() {
    app()
        .run_ruby(
            r##"
widget = Widget.create!(name: "alpha")
Part.create!(name: "p", widget_id: widget.id)
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets.json", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
raise "status #{status} #{text}" unless status == 200
raise "body #{text}" unless text == %([{"id":1,"name":"alpha","preloaded":true}])
puts "ALL OK"
"##,
        )
        .assert_passes();
}

/// A model method's untyped receiver keeps the Rails spelling, as
/// before: the widening is for views and tests only.
#[test]
fn a_model_method_receiver_is_not_widened() {
    let (emitted, _errors) = app()
        .write(
            "app/models/part.rb",
            "class Part < ApplicationRecord\n  belongs_to :widget, optional: true\n  belongs_to :gadget, optional: true\n\n  def self.check(owner)\n    owner.parts.loaded?\n  end\nend\n",
        )
        .emit(BuildTarget::Ruby);
    let source = std::fs::read_to_string(emitted.join("app/models/part.rb")).expect("part.rb emitted");
    assert!(source.contains("owner.parts.loaded?"), "a model method's untyped receiver must stay:\n{source}");
}
