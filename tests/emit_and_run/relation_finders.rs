use super::emit_and_run;

/// Build the same lookup through each receiver-preservation path: inline,
/// assigned to a local/ivar, or returned from a controller helper. The
/// optional association makes a successful lookup distinguish NULL from a row.
fn app(finder: &str, receiver: &str) -> emit_and_run::Overlay {
    let lookup = match receiver {
        "inline" => format!("widget = Widget.includes(:category).{finder}(id: params[:id])"),
        "local" => format!("widgets = Widget.includes(:category)\n    widget = widgets.{finder}(id: params[:id])"),
        "ivar" => format!("@widgets = Widget.includes(:category)\n    widget = @widgets.{finder}(id: params[:id])"),
        "helper" => format!("widget = widget_scope.{finder}(id: params[:id])"),
        "explicit_helper" => format!("widget = self.widget_scope.{finder}(id: params[:id])"),
        _ => panic!("unknown receiver: {receiver}"),
    };
    let helper = if matches!(receiver, "helper" | "explicit_helper") {
        "\n  def widget_scope\n    Widget.includes(:category)\n  end\n"
    } else {
        ""
    };
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "categories", force: :cascade do |t|
    t.string "name", null: false
  end
  create_table "widgets", force: :cascade do |t|
    t.string "name", null: false
    t.integer "category_id"
  end
end
"#)
        .write("app/models/category.rb", "class Category < ApplicationRecord\n  has_many :widgets\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  belongs_to :category, optional: true\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/widgets/:id\", to: \"widgets#show\"\nend\n")
        .write("app/controllers/widgets_controller.rb", &format!(r#"class WidgetsController < ApplicationController
  def show
    {lookup}
    if widget
      category = widget.category
      render plain: widget.name + "/" + (category ? category.name : "none")
    else
      render plain: "missing"
    end
  end
{helper}
end
"#))
}

const ASSERTIONS: &str = r#"
category = Category.create!(name: "attached")
first = Widget.create!(name: "first", category_id: category.id)
second = Widget.create!(name: "second", category_id: nil)
[[first.id, "first/attached"], [second.id, "second/none"]].each do |id, expected|
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/#{id}", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
  raise "status #{status}" unless status == 200
  raise "wrong finder result: #{body.join}" unless body.join == expected
end
"#;

#[test]
fn includes_find_by_preserves_its_relation_receiver() {
    assert_finder("find_by", "inline");
}

/// Execute present/NULL association lookups, then check the terminal's
/// missing-record contract: a nil result for find_by versus a 404 for find_by!.
fn assert_finder(finder: &str, receiver: &str) {
    let missing = if finder == "find_by!" {
        "raise \"missing record must be 404\" unless status == 404"
    } else {
        "raise \"missing record result\" unless status == 200 && body.join == \"missing\""
    };
    app(finder, receiver)
        .run_ruby(&format!(r#"{ASSERTIONS}
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/999", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
{missing}
"#))
        .assert_passes();
}

#[test]
fn includes_find_by_bang_preserves_its_relation_receiver() {
    assert_finder("find_by!", "inline");
}

#[test]
fn relation_finders_preserve_local_receivers() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "local");
    }
}

#[test]
fn relation_finders_preserve_ivar_receivers() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "ivar");
    }
}

#[test]
fn relation_finders_preserve_helper_return_values() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "helper");
    }
}

#[test]
fn relation_finders_preserve_explicit_self_helper_return_values() {
    for finder in ["find_by", "find_by!"] {
        assert_finder(finder, "explicit_helper");
    }
}

/// Shapes 1 and 2 of #558: `Part.includes(:widget).named("b").first` (an
/// app scope on a class chain) and `Part.includes(:widget).find_by_id(id)`
/// (a dynamic finder on a class chain) both hydrated the chain's receiver
/// into an Array before the trailing call ran, because the arel pass's
/// relation-receiver predicate didn't know the app's own scopes and
/// didn't treat `find_by_<attr>` as a finder. Controls alongside: a plain
/// has_many reader chained with `where`/`order`, and `.new` through a
/// helper-returned owner — both already worked and must keep working.
fn scope_and_dynamic_finder_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
  create_table "parts", force: :cascade do |t|
    t.string "name"
    t.integer "widget_id"
  end
end
"#)
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\n  has_many :parts\nend\n")
        .write("app/models/part.rb", "class Part < ApplicationRecord\n  belongs_to :widget\n  scope :named, ->(n) { where(name: n) }\nend\n")
        .write("config/routes.rb", r#"Rails.application.routes.draw do
  get "/widgets/:id/a_where", to: "widgets#a_where"
  get "/widgets/:id/a_new", to: "widgets#a_new"
  get "/parts/b_scope", to: "widgets#b_scope"
  get "/parts/:id/b_dyn", to: "widgets#b_dyn"
end
"#)
        .write("app/controllers/widgets_controller.rb", r#"class WidgetsController < ApplicationController
  # Control: plain has_many reader on a local, then where/order.
  def a_where
    widget = Widget.find(params[:id])
    parts = widget.parts.where(name: "b").order(:name)
    render plain: parts.map(&:name).join(",")
  end

  # Control: plain has_many reader through a helper, then new.
  def a_new
    part = current_widget.parts.new(name: "z")
    render plain: part.widget_id.to_s
  end

  # Shape 1: class chain, then an app scope.
  def b_scope
    part = Part.includes(:widget).named("b").first
    render plain: part.name
  end

  # Shape 2: class chain, then a dynamic finder.
  def b_dyn
    part = Part.includes(:widget).find_by_id(params[:id])
    render plain: part.name
  end

  private

  def current_widget
    Widget.find(params[:id])
  end
end
"#)
}

fn scope_and_dynamic_finder_assertions() -> &'static str {
    r#"
require_relative "app/controllers/widgets_controller"
widget = Widget.create!(name: "w1")
a_part = Part.create!(widget: widget, name: "a")
b_part = Part.create!(widget: widget, name: "b")

controller = WidgetsController.new
controller.params = {"id" => widget.id.to_s}
controller.process_action(:a_where)
raise "control a_where: #{controller.body}" unless controller.body == "b"

controller = WidgetsController.new
controller.params = {"id" => widget.id.to_s}
controller.process_action(:a_new)
raise "control a_new: #{controller.body}" unless controller.body == widget.id.to_s

controller = WidgetsController.new
controller.process_action(:b_scope)
raise "shape 1 (app scope on a class chain): #{controller.body}" unless controller.body == "b"

controller = WidgetsController.new
controller.params = {"id" => b_part.id.to_s}
controller.process_action(:b_dyn)
raise "shape 2 (dynamic finder on a class chain): #{controller.body}" unless controller.body == "b"

puts "relation chain scope and dynamic finder passed"
"#
}

#[test]
fn relation_chain_app_scope_and_dynamic_finder_run() {
    scope_and_dynamic_finder_app()
        .run_ruby(scope_and_dynamic_finder_assertions())
        .assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn relation_chain_app_scope_and_dynamic_finder_run_on_spinel() {
    let script = format!(
        "Db.configure(\":memory:\")\nSchema.statements.each {{ |sql| Db.exec(sql) }}\nActiveRecord.adapter = SqliteAdapter\n{}",
        scope_and_dynamic_finder_assertions()
    );
    scope_and_dynamic_finder_app().run_spinel(&script).assert_passes();
}
