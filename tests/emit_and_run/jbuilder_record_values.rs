//! Records in jbuilder templates, run as the emitted app renders them
//! and compared with what Rails 8.1.4 + jbuilder 2.15.1 answer for the
//! same rows and templates.
//!
//! `json.partial!(record, …)` renders the partial of the record's class
//! (`record.to_partial_path`), whatever the local holding it is called.
//!
//! A record as a value is encoded with `as_json`: its attributes, and
//! a collection of records an array of them.

use super::emit_and_run;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "size"
  end
  create_table "gadgets", force: :cascade do |t|
    t.string "label"
    t.integer "widget_id"
    t.string "note"
  end
end
"#;

const WIDGETS_CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  helper_method :current_widget

  def show
    @widget = Widget.find(params[:id])
    @gadgets = @widget.gadgets.to_a
    @gadget = @gadgets.first
    @item = Widget.find(params[:id])
  end

  def assoc
    @widget = Widget.find(params[:id])
    @gadget = @widget.gadgets.first
  end

  private

  def current_widget
    Widget.first
  end
end
"#;

const API_WIDGETS_CONTROLLER: &str = r#"class Api::WidgetsController < ApplicationController
  def show
    @widget = Widget.find(params[:id])
    @gadget = @widget.gadgets.first
  end
end
"#;

/// The app every test here starts from: a widget with gadgets, and a
/// partial for each model (plus a namespaced one for the gadget).
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", SCHEMA)
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "widgets/:id", to: "widgets#show", defaults: { format: :json }
  get "widgets/:id/assoc", to: "widgets#assoc", defaults: { format: :json }
  namespace :api do
    get "widgets/:id", to: "widgets#show", defaults: { format: :json }
  end
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :gadgets\nend\n")
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  belongs_to :widget, optional: true\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER)
        .write("app/controllers/api/widgets_controller.rb", API_WIDGETS_CONTROLLER)
        .write("app/views/gadgets/_gadget.json.jbuilder", "json.label gadget.label\n")
        .write("app/views/widgets/_widget.json.jbuilder", "json.name widget.name\n")
        .write("app/views/api/gadgets/_gadget.json.jbuilder", "json.api_label gadget.label\n")
}

const SEED: &str = r#"
require_relative "app/controllers/widgets_controller"
widget = Widget.create!(name: "alpha", size: 3)
Gadget.create!(label: "g1", widget_id: widget.id)
def render_action(controller, action, id)
  controller.params = {"id" => id.to_s}
  controller.process_action(action)
  controller.body
end
"#;

/// The record's class picks the partial: a block parameter that is not
/// named after the model, an ivar named something else, a helper
/// method, an association reader (plain and through `&.`), a finder on
/// the model, and `partial: …, object:` all render the model's
/// partial, which a `partial:` option after a record does not change.
/// A namespaced controller's view renders the namespaced partial. A key
/// whose block renders the partial only under a condition is left out
/// when the condition fails (`extra`), as Jbuilder leaves out a key
/// whose block sets nothing: the unresolved partial used to keep the
/// key as `"extra":{}`.
#[test]
fn a_records_partial_comes_from_its_class_not_its_local_name() {
    let show = r#"json.items(@gadgets) do |gadget|
  json.partial!(gadget, partial: "gadgets/gadget", as: :gadget)
end
json.named(@gadgets) do |g|
  json.partial!(g, partial: "gadgets/gadget", as: :gadget)
end
json.odd(@gadgets) do |g|
  json.partial!(g, partial: "gadgets/missing", as: :gadget)
end
json.item do
  json.partial!(@item)
end
json.helper do
  json.partial!(current_widget)
end
json.owner do
  json.partial!(@gadget.widget, as: :widget)
end
json.safe do
  json.partial!(@gadget&.widget, as: :widget)
end
json.found do
  json.partial!(Widget.first, as: :widget)
end
json.kw do
  json.partial! partial: "gadgets/gadget", object: @gadget, as: :gadget
end
json.extra do
  json.partial!(@gadget.widget, as: :widget) if @gadget.note
end
json.kept do
  json.partial!(@gadget.widget, as: :widget) if @gadget.label
end
json.chain(@widget&.gadgets) do |g|
  json.partial!("gadgets/with_owner", gadget: g)
end
"#;
    // Rails 8.1.4 + jbuilder 2.15.1, same rows:
    //   GET /widgets/1     => the first line below
    //   GET /api/widgets/1 => the second
    let script = format!(
        r#"{SEED}
expected = '{{"items":[{{"label":"g1"}}],"named":[{{"label":"g1"}}],"odd":[{{"label":"g1"}}],"item":{{"name":"alpha"}},"helper":{{"name":"alpha"}},"owner":{{"name":"alpha"}},"safe":{{"name":"alpha"}},"found":{{"name":"alpha"}},"kw":{{"label":"g1"}},"kept":{{"name":"alpha"}},"chain":[{{"label":"g1","owner":{{"name":"alpha"}}}}]}}'
actual = render_action(WidgetsController.new, :show, widget.id)
raise actual.inspect unless actual == expected
expected = '{{"gadget":{{"api_label":"g1"}}}}'
require_relative "app/controllers/api/widgets_controller"
actual = render_action(Api::WidgetsController.new, :show, widget.id)
raise actual.inspect unless actual == expected
puts "a record's partial comes from its class"
"#
    );
    // A partial's own local through `&.`, which arrives as a temporary
    // (`(__safe_nav = gadget) && __safe_nav.widget`).
    let with_owner = "json.label gadget.label\njson.owner do\n  json.partial!(gadget&.widget, as: :widget)\nend\n";
    app()
        .write("app/views/widgets/show.json.jbuilder", show)
        .write("app/views/gadgets/_with_owner.json.jbuilder", with_owner)
        .write("app/views/api/widgets/show.json.jbuilder", "json.gadget do\n  json.partial!(@gadget, as: :gadget)\nend\n")
        .run_ruby(&script)
        .assert_passes();
}

/// A record as a value (an association named in `json.call` /
/// `json.extract!`, or a pair's value) is its attributes, as `as_json`
/// gives them, and a collection of records is an array of those. It was
/// the record's `to_s`, `"#<Widget:0x…>"`.
#[test]
fn a_record_value_is_its_attributes_not_its_to_s() {
    let assoc = r#"json.call(@gadget, :label, :widget)
json.extract!(@widget, :name, :gadgets)
json.owner @gadget.widget
json.list @widget.gadgets.to_a
json.none @gadget.note
"#;
    // Rails 8.1.4 + jbuilder 2.15.1, same rows: GET /widgets/1/assoc.
    let script = format!(
        r#"{SEED}
expected = '{{"label":"g1","widget":{{"id":1,"name":"alpha","size":3}},"name":"alpha","gadgets":[{{"id":1,"label":"g1","widget_id":1,"note":null}}],"owner":{{"id":1,"name":"alpha","size":3}},"list":[{{"id":1,"label":"g1","widget_id":1,"note":null}}],"none":null}}'
actual = render_action(WidgetsController.new, :assoc, widget.id)
raise actual.inspect unless actual == expected
puts "a record value is its attributes"
"#
    );
    app()
        .write("app/views/widgets/show.json.jbuilder", "json.id @widget.id\n")
        .write("app/views/widgets/assoc.json.jbuilder", assoc)
        .run_ruby(&script)
        .assert_passes();
}

/// The tags app: a `Tag` whose polymorphic `thing` is a `Widget` or a
/// `Gadget` (the targets the app declares through `has_many …, as:`),
/// a partial for each, and the seeded rows with a renderer for
/// `TagsController`.
fn tags_app() -> emit_and_run::Overlay {
    let schema = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "size"
  end
  create_table "gadgets", force: :cascade do |t|
    t.string "label"
  end
  create_table "tags", force: :cascade do |t|
    t.string "label"
    t.string "thing_type"
    t.integer "thing_id"
  end
end
"#;
    let controller = r#"class TagsController < ApplicationController
  def show
    @tag = Tag.find(params[:id])
  end

  def pick
    @tag = Tag.find(params[:id])
    @thing = @tag.label == "w" ? Widget.first : Gadget.first
  end
end
"#;
    emit_and_run::empty_app()
        .write("db/schema.rb", schema)
        .write(
            "config/routes.rb",
            r#"Rails.application.routes.draw do
  get "tags/:id", to: "tags#show", defaults: { format: :json }
  get "tags/:id/pick", to: "tags#pick", defaults: { format: :json }
end
"#,
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :tags, as: :thing\nend\n")
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  has_many :tags, as: :thing\nend\n")
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\n  belongs_to :thing, polymorphic: true\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("app/controllers/tags_controller.rb", controller)
        .write("app/views/widgets/_widget.json.jbuilder", "json.name widget.name\n")
        .write("app/views/gadgets/_gadget.json.jbuilder", "json.label gadget.label\n")
        .write("app/views/tags/show.json.jbuilder", "json.id @tag.id\n")
        .write("app/views/tags/pick.json.jbuilder", "json.id @tag.id\n")
}

const TAGS_SEED: &str = r#"
require_relative "app/controllers/tags_controller"
widget = Widget.create!(name: "alpha", size: 3)
gadget = Gadget.create!(label: "g1")
w = Tag.create!(label: "w", thing_type: "Widget", thing_id: widget.id)
g = Tag.create!(label: "g", thing_type: "Gadget", thing_id: gadget.id)
def render_action(action, id)
  controller = TagsController.new
  controller.params = {"id" => id.to_s}
  controller.process_action(action)
  controller.body
end
"#;

/// A polymorphic reader, named in `json.call` or given as a pair's
/// value, is a record of one of the reader's targets, written by a
/// switch on its class: the attributes of the row's class. The first
/// was refused, the second written as the record's `to_s`.
#[test]
fn a_polymorphic_record_value_is_its_rows_class_attributes() {
    // Rails 8.1.4 + jbuilder 2.15.1, same rows: GET /tags/1, /tags/2.
    let script = format!(
        r#"{TAGS_SEED}
expected = '{{"label":"w","thing":{{"id":1,"name":"alpha","size":3}},"owner":{{"id":1,"name":"alpha","size":3}}}}'
actual = render_action(:show, w.id)
raise actual.inspect unless actual == expected
expected = '{{"label":"g","thing":{{"id":1,"label":"g1"}},"owner":{{"id":1,"label":"g1"}}}}'
actual = render_action(:show, g.id)
raise actual.inspect unless actual == expected
puts "a polymorphic record value is its row's class's attributes"
"#
    );
    tags_app()
        .write("app/views/tags/show.json.jbuilder", "json.call(@tag, :label, :thing)\njson.owner @tag.thing\n")
        .run_ruby(&script)
        .assert_passes();
}

/// `json.partial!` of a record typed as a union of two models renders
/// the partial of the record's class, by a switch on it. It was
/// refused.
#[test]
fn a_partial_over_a_union_of_models_is_its_records_class_partial() {
    // Rails 8.1.4 + jbuilder 2.15.1, same rows: GET /tags/1/pick, /tags/2/pick.
    let script = format!(
        r#"{TAGS_SEED}
expected = '{{"id":1,"picked":{{"name":"alpha"}}}}'
actual = render_action(:pick, w.id)
raise actual.inspect unless actual == expected
expected = '{{"id":2,"picked":{{"label":"g1"}}}}'
actual = render_action(:pick, g.id)
raise actual.inspect unless actual == expected
puts "a partial over a union is its record's class's partial"
"#
    );
    tags_app()
        .write("app/views/tags/pick.json.jbuilder", "json.id @tag.id\njson.picked do\n  json.partial! @thing\nend\n")
        .run_ruby(&script)
        .assert_passes();
}

/// A polymorphic reader with one declared target is still a record of
/// whatever class its row names. The class is checked, as it is for two
/// targets: a row of another class raises rather than be rendered by
/// the target's partial, which would read attributes that record does
/// not have (`widgets/_widget` called on a `Gadget` row: `undefined
/// method 'name'`). Rails renders the row's own partial; a class
/// outside the declared targets is the refusal the lowering raises.
#[test]
fn a_partial_over_a_one_target_polymorphic_reader_checks_the_rows_class() {
    let schema = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "gadgets", force: :cascade do |t|
    t.string "label"
  end
  create_table "tags", force: :cascade do |t|
    t.string "thing_type"
    t.integer "thing_id"
  end
end
"#;
    let script = r#"
require_relative "app/controllers/tags_controller"
widget = Widget.create!(name: "alpha")
gadget = Gadget.create!(label: "g1")
w = Tag.create!(thing_type: "Widget", thing_id: widget.id)
g = Tag.create!(thing_type: "Gadget", thing_id: gadget.id)
def render_action(id)
  controller = TagsController.new
  controller.params = {"id" => id.to_s}
  controller.process_action(:show)
  controller.body
end
# Rails 8.1.4 + jbuilder 2.15.1, same row: GET /tags/1.
expected = '{"thing":{"name":"alpha"}}'
actual = render_action(w.id)
raise actual.inspect unless actual == expected
begin
  actual = render_action(g.id)
rescue => e
  raise e unless e.message.include?("of a class other than Widget")
  puts "a one-target polymorphic reader checks the row's class"
  exit
end
raise "rendered a Gadget row with the Widget partial: #{actual}"
"#;
    emit_and_run::empty_app()
        .write("db/schema.rb", schema)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"tags/:id\", to: \"tags#show\", defaults: { format: :json }\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :tags, as: :thing\nend\n")
        .write("app/models/gadget.rb", "class Gadget < ApplicationRecord\nend\n")
        .write("app/models/tag.rb", "class Tag < ApplicationRecord\n  belongs_to :thing, polymorphic: true\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "app/controllers/tags_controller.rb",
            "class TagsController < ApplicationController\n  def show\n    @tag = Tag.find(params[:id])\n  end\nend\n",
        )
        .write("app/views/widgets/_widget.json.jbuilder", "json.name widget.name\n")
        .write("app/views/gadgets/_gadget.json.jbuilder", "json.label gadget.label\n")
        .write(
            "app/views/tags/show.json.jbuilder",
            "json.thing do\n  json.partial! @tag.thing\nend\n",
        )
        .run_ruby(script)
        .assert_passes();
}
