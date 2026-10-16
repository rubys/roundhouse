//! An enum mapping with a `nil` value (`unknown: nil`) reads its labels,
//! and a NULL column reads as the `nil` entry's label.
//!
//! `enum_mapping_reads_label` accepted only all-Integer (or all-String)
//! values, and ingest did not count a `nil` value as a literal, so
//! `enum :kind, { small: 0, large: 1, unknown: nil }` silently fell back
//! to "reads the stored value": `widget.kind` answered `1` and `nil`
//! where Rails answers `"large"` and `"unknown"` (`EnumType#deserialize`
//! is `mapping.key(value)`, and `mapping.key(nil)` is the `nil` entry's
//! label). The `nil` entry is the usual way to name a NULL column.
//!
//! Now the reader answers the label, its NULL branch answers the `nil`
//! entry's label, and writing that label stores NULL.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

/// The model, with a `where` by label in app code, where the lowering
/// sees it (a runner script's `where` reaches the runtime as written).
fn widget_model(mapping: &str) -> String {
    format!(
        "class Widget < ApplicationRecord\n  enum :kind, {mapping}\n\n  def self.unknown_count\n    where(kind: :unknown).count\n  end\nend\n"
    )
}

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\", null: false\n    t.integer \"kind\"\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n")
        .write("app/models/widget.rb", &widget_model("{ small: 0, large: 1, unknown: nil }"))
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.all\n  end\nend\n",
        )
        .write(
            "app/views/widgets/index.json.jbuilder",
            "json.array!(@widgets) do |widget|\n  json.call(widget, :id, :kind)\nend\n",
        )
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: [:index]\nend\n")
}

#[test]
fn nil_valued_enum_reader_answers_labels() {
    let (emitted, _errors) = app().emit(BuildTarget::Spinel);
    let model = std::fs::read_to_string(emitted.join("app/models/widget.rb")).expect("widget.rb emitted");
    assert!(
        model.contains(r#"ActiveRecord.enum_label(@kind, ["small", "large"], [0, 1])"#),
        "the reader must map the stored value to its label:\n{model}"
    );
    assert!(model.contains(r#""unknown""#), "the NULL branch must answer the nil entry's label:\n{model}");
}

#[test]
fn nil_valued_enum_runs_with_rails_answers() {
    app()
        .run_ruby(
            r##"
large = Widget.create!(name: "a", kind: 1)
blank = Widget.create!(name: "b")
raise "large #{large.kind.inspect}" unless large.kind == "large"
raise "blank #{blank.kind.inspect}" unless blank.kind == "unknown"
raise "unknown? #{blank.unknown?}" unless blank.unknown? && !large.unknown?
large.kind = "unknown"
raise "written #{large.instance_variable_get(:@kind).inspect}" unless large.instance_variable_get(:@kind).nil? && large.kind == "unknown"
large.kind = "small"
raise "small #{large.instance_variable_get(:@kind).inspect}" unless large.instance_variable_get(:@kind) == 0
raise "scope #{Widget.unknown.count}" unless Widget.unknown.count == 1
raise "where #{Widget.unknown_count}" unless Widget.unknown_count == 1
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets.json", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
raise "status #{status} #{text}" unless status == 200
raise "body #{text}" unless text == %([{"id":1,"kind":"large"},{"id":2,"kind":"unknown"}])
puts "ALL OK"
"##,
        )
        .assert_passes();
}

/// A mapping whose only entry is `nil` (`enum :kind, { unknown: nil }`):
/// Rails 8.1 reads NULL as `"unknown"` (and any stored value no label
/// names as nil), writes `"unknown"` as NULL, refuses any other value
/// (`'1' is not a valid kind`), and `Widget.unknown` / `where(kind:
/// :unknown)` select `kind IS NULL`.
#[test]
fn nil_only_enum_reads_and_writes_its_label() {
    app()
        .write("app/models/widget.rb", &widget_model("{ unknown: nil }"))
        .run_ruby(
            r##"
blank = Widget.create!(name: "b")
raise "blank #{blank.kind.inspect}" unless blank.kind == "unknown"
raise "unknown? #{blank.unknown?}" unless blank.unknown?
blank.kind = "unknown"
raise "written #{blank.instance_variable_get(:@kind).inspect}" unless blank.instance_variable_get(:@kind).nil? && blank.kind == "unknown"
refused = begin
  blank.kind = "1"
  false
rescue ArgumentError
  true
end
raise "a value no label names must be refused" unless refused
raise "scope #{Widget.unknown.count}" unless Widget.unknown.count == 1
raise "where #{Widget.unknown_count}" unless Widget.unknown_count == 1
raise "kinds #{Widget.kinds.inspect}" unless Widget.kinds == { "unknown" => nil }
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets.json", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
raise "status #{status} #{text}" unless status == 200
raise "body #{text}" unless text == %([{"id":1,"kind":"unknown"}])
puts "ALL OK"
"##,
        )
        .assert_passes();
}
