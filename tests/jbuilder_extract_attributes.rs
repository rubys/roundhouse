//! `json.extract!(record, *attrs)` / `json.(record, *attrs)`, and
//! `json.array!(collection, *attrs)`.
//!
//! Jbuilder's `_extract_method_values` writes `record.public_send(attr)`
//! under each name exactly as given, Symbol or String, however the list
//! was built. The lowering accepted only Symbol literals: any other
//! argument (a splat, a param, an ivar, a constant, a String literal)
//! classified the statement as unknown, and an unknown statement is
//! written as nothing, so the response was a 200 with the keys missing
//! and nothing reported it. `array!` with attribute names did the same
//! and rendered `{}`.
//!
//! Expected values are Rails 8.1.4 with jbuilder 2.15.1 on the same
//! templates, with row 1 `name: "alpha", size: 3` and row 2
//! `name: "beta", size: 5`.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ACTIONS: &[(&str, &str)] = &[
    ("splat_params", "json.call(@widget, *params[:fields])"),
    ("splat_const", "json.extract!(@widget, *Widget::PUBLIC_ATTRS)"),
    ("splat_ivar", "json.extract!(@widget, *@attrs)"),
    ("single_dynamic", "json.call(@widget, params[:field])"),
    ("mixed", "json.extract!(@widget, :id, *Widget::PUBLIC_ATTRS)"),
    ("string_literal", "json.extract!(@widget, \"name\", \"size\")"),
    ("literal", "json.extract!(@widget, :name, :size)"),
];

fn app() -> emit_and_run::Overlay {
    let mut controller = String::from("class WidgetsController < ApplicationController\n");
    let mut routes = String::from("Rails.application.routes.draw do\n");
    let mut overlay = emit_and_run::empty_app();
    for (action, line) in ACTIONS {
        let ivars = if *action == "splat_ivar" { "\n    @attrs = %i[name size]" } else { "" };
        controller.push_str(&format!(
            "  def {action}\n    @widget = Widget.find(params[:id]){ivars}\n  end\n\n"
        ));
        routes.push_str(&format!(
            "  get \"widgets/:id/{action}\", to: \"widgets#{action}\", defaults: {{ format: :json }}\n"
        ));
        overlay = overlay.write(
            &format!("app/views/widgets/{action}.json.jbuilder"),
            &format!("json.title \"t\"\n{line}\njson.done true\n"),
        );
    }
    controller.push_str(concat!(
        "  def index\n    @widgets = Widget.order(:id).to_a\n  end\n\n",
        "  def stamps\n    @widgets = Widget.order(:id).to_a\n  end\n\n",
        "  def rows\n    @rows = [{ name: \"a\" }, { name: \"b\" }]\n  end\n\n",
        "  def mixed_options\n    @widgets = Widget.order(:id).to_a\n  end\n\n",
        "  def partial_list\n    @widgets = Widget.order(:id).to_a\n  end\nend\n",
    ));
    routes.push_str(concat!(
        "  get \"widgets\", to: \"widgets#index\", defaults: { format: :json }\n",
        "  get \"stamps\", to: \"widgets#stamps\", defaults: { format: :json }\n",
        "  get \"rows\", to: \"widgets#rows\", defaults: { format: :json }\n",
        "  get \"mixed_options\", to: \"widgets#mixed_options\", defaults: { format: :json }\n",
        "  get \"partial_list\", to: \"widgets#partial_list\", defaults: { format: :json }\nend\n",
    ));
    overlay
        .write(
            "app/views/widgets/index.json.jbuilder",
            "json.array!(@widgets, :name, :size)\n",
        )
        .write(
            "app/views/widgets/stamps.json.jbuilder",
            "json.array!(@widgets, :name, :created_at, :due_on)\n",
        )
        .write("app/views/widgets/rows.json.jbuilder", "json.array!(@rows, :name)\n")
        .write(
            "app/views/widgets/mixed_options.json.jbuilder",
            "json.array!(@widgets, :name, fields: :size)\n",
        )
        .write(
            "app/views/widgets/partial_list.json.jbuilder",
            "json.array!(@widgets, partial: \"widgets/widget\", as: :widget)\n",
        )
        .write("app/views/widgets/_widget.json.jbuilder", "json.extract! widget, :name, :size\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/models/widget.rb",
            "class Widget < ApplicationRecord\n  PUBLIC_ATTRS = %i[name size].freeze\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n    t.integer :size\n    t.date :due_on\n    t.timestamps\n  end\nend\n",
        )
        .write("app/controllers/widgets_controller.rb", &controller)
        .write("config/routes.rb", &routes)
}

const PROBE: &str = r##"
def probe(path, query = "")
  status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => path, "QUERY_STRING" => query, "rack.input" => StringIO.new(""))
  text = +""
  body.each { |part| text << part }
  [status, text]
end

def refusal(path, query = "")
  probe(path, query)
  nil
rescue RuntimeError => e
  e.message
end

Widget.create!(name: "alpha", size: 3)
Widget.create!(name: "beta", size: 5)
"##;

/// A list the lowering cannot read at compile time is refused where it
/// stands, the way a non-lowerable `merge!` is, instead of rendering a
/// 200 with the keys missing.
#[test]
fn jbuilder_extract_with_a_dynamic_attribute_list_is_refused() {
    app()
        .run_ruby(&format!(
            r##"{PROBE}
[
  ["/widgets/1/splat_params", "fields[]=name&fields[]=size"],
  ["/widgets/1/splat_const", ""],
  ["/widgets/1/splat_ivar", ""],
  ["/widgets/1/single_dynamic", "field=name"],
  ["/widgets/1/mixed", ""],
].each do |path, query|
  message = refusal(path, query)
  raise "#{{path}} was not refused: #{{probe(path, query).inspect}}" if message.nil?
  raise "#{{path}}: #{{message}}" unless message.include?("extract! with dynamic attributes not supported")
end
got = probe("/widgets/1/literal")
raise "literal #{{got.inspect}}" unless got == [200, %({{"title":"t","name":"alpha","size":3,"done":true}})]
puts "ALL OK"
"##
        ))
        .assert_passes();
}

/// A String literal names its key exactly as a Symbol does, and
/// `array!(collection, *names)` writes those pairs for each element.
#[test]
fn jbuilder_string_names_and_array_attributes_render_as_rails_does() {
    app()
        .run_ruby(&format!(
            r##"{PROBE}
got = probe("/widgets/1/string_literal")
raise "string_literal #{{got.inspect}}" unless got == [200, %({{"title":"t","name":"alpha","size":3,"done":true}})]
got = probe("/widgets")
raise "array! #{{got.inspect}}" unless got == [200, %([{{"name":"alpha","size":3}},{{"name":"beta","size":5}}])]
Widget.delete_all
got = probe("/widgets")
raise "empty array! #{{got.inspect}}" unless got == [200, %([])]
puts "ALL OK"
"##
        ))
        .assert_passes();
}

/// The unit half: a String name whose `record.<name>` read could not be
/// spelled is refused with the rest, never written as nothing.
#[test]
fn jbuilder_a_string_name_that_is_not_a_method_name_is_refused() {
    let (emitted, _errors) = app()
        .write(
            "app/views/widgets/literal.json.jbuilder",
            "json.title \"t\"\njson.extract!(@widget, \"first-name\")\njson.done true\n",
        )
        .emit(roundhouse::project::BuildTarget::Ruby);
    let view = std::fs::read_to_string(emitted.join("app/views/widgets/literal_json.rb")).expect("view");
    assert!(view.contains("extract! with dynamic attributes not supported"), "{view}");
}

/// An `array!` element's temporal columns render as the template
/// argument's do: Rails' `"2026-10-16T04:07:33.320Z"` and
/// `"2026-10-16"`, not `Time#to_s`. A collection whose elements are not
/// known to be records (Jbuilder reads a Hash element with `fetch`) is
/// refused rather than read by method, which raised `NoMethodError`.
#[test]
fn jbuilder_array_attributes_read_temporal_columns_and_refuse_untyped_elements() {
    app()
        .run_ruby(&format!(
            r##"{PROBE}
Widget.find(1).update!(due_on: "2026-10-16")
status, body = probe("/stamps")
raise "stamps status #{{status}}" unless status == 200
stamp = /"created_at":"\d{{4}}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{{3}}Z"/
pattern = /\A\[\{{"name":"alpha",#{{stamp.source}},"due_on":"2026-10-16"\}},\{{"name":"beta",#{{stamp.source}},"due_on":null\}}\]\z/
raise "stamps #{{body}}" unless body.match?(pattern)
message = refusal("/rows")
raise "rows was not refused: #{{probe("/rows").inspect}}" if message.nil?
raise "rows: #{{message}}" unless message.include?("array! with attribute names over an untyped collection not supported")
puts "ALL OK"
"##
        ))
        .assert_passes();
}

/// A Hash beside the attribute names that is not `partial:` + `as:`
/// options is an attribute to Jbuilder, which raises `TypeError ({fields:
/// :size} is not a symbol nor a string)` for a non-empty collection. It
/// is refused, not rendered as `{}`. Real partial options still render
/// through the partial.
#[test]
fn jbuilder_array_with_a_non_option_hash_is_refused_and_partial_options_still_render() {
    app()
        .run_ruby(&format!(
            r##"{PROBE}
message = refusal("/mixed_options")
raise "mixed_options was not refused: #{{probe("/mixed_options").inspect}}" if message.nil?
raise "mixed_options: #{{message}}" unless message.include?("array! with a Hash that is not partial options not supported")
got = probe("/partial_list")
raise "partial_list #{{got.inspect}}" unless got == [200, %([{{"name":"alpha","size":3}},{{"name":"beta","size":5}}])]
puts "ALL OK"
"##
        ))
        .assert_passes();
}
