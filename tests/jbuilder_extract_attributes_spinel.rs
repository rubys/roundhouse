//! `json.extract!` with String names and `json.array!(collection,
//! *names)`, compiled by Spinel. The CRuby side is
//! `jbuilder_extract_attributes.rs`; the expected bodies are jbuilder
//! 2.15.1's.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn jbuilder_string_names_and_array_attributes_under_spinel() {
    let overlay = emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define(version: 1) do\n  create_table :widgets do |t|\n    t.string :name\n    t.integer :size\n  end\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def index\n    @widgets = Widget.all\n  end\n\n  def show\n    @widget = Widget.find(params[:id])\n  end\nend\n",
        )
        .write("app/views/widgets/index.json.jbuilder", "json.array!(@widgets, :name, :size)\n")
        .write(
            "app/views/widgets/show.json.jbuilder",
            "json.title \"t\"\njson.extract!(@widget, \"name\", \"size\")\njson.done true\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :widgets, only: [:index, :show]\nend\n",
        );
    let run = overlay.run_spinel(
        r#"
a = Widget.new(name: "alpha", size: 3)
b = Widget.new(name: "beta", size: 5)
got = Views::Widgets.index_json([a, b])
raise "array!: #{got}" unless got == "[{\"name\":\"alpha\",\"size\":3},{\"name\":\"beta\",\"size\":5}]"
got = Views::Widgets.show_json(a)
raise "extract!: #{got}" unless got == "{\"title\":\"t\",\"name\":\"alpha\",\"size\":3,\"done\":true}"
puts "ok"
"#,
    );
    run.assert_passes();
}
