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
