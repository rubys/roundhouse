//! `with_indifferent_access` on an untyped receiver lowers to
//! `ActiveSupport.indifferent(recv)`, as it does on a Hash-typed one.
//!
//! A json column reads as `untyped`, and `untyped || {}` stays untyped,
//! so `(widget.extras || {}).with_indifferent_access` was emitted as the
//! call itself, which no runtime defines: the request raised
//! `NoMethodError: undefined method 'with_indifferent_access' for an
//! instance of Hash`. Reading a json column and making it indifferent is
//! the most common way a Rails app calls it. The result is typed
//! indifferent, so a call chained on it converts its Symbol keys.
//!
//! Rails answers `alpha,k,seen|v|r-9` for the request below.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const WIDGETS_CONTROLLER: &str = r##"class WidgetsController < ApplicationController
  def update
    widget = Widget.find(params[:id])
    extras = (widget.extras || {}).with_indifferent_access.merge!({ widget.name => params[:ref] })
    extras.merge!(seen: true)
    render plain: "#{extras.keys.sort.join(",")}|#{extras[:k]}|#{extras["alpha"]}"
  end
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\", null: false\n    t.json \"extras\", default: {}\n  end\nend\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write("app/controllers/widgets_controller.rb", WIDGETS_CONTROLLER)
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: [:update]\nend\n")
}

#[test]
fn untyped_receiver_lowers_to_the_runtime_conversion() {
    let (emitted, _errors) = app().emit(BuildTarget::Spinel);
    let source = std::fs::read_to_string(emitted.join("app/controllers/widgets_controller.rb"))
        .expect("widgets_controller.rb emitted");
    assert!(
        !source.contains(".with_indifferent_access"),
        "the call is left for a runtime that does not define it:\n{source}"
    );
    assert!(
        source.contains("ActiveSupport.indifferent(widget.extras || {})"),
        "expected the receiver converted by ActiveSupport.indifferent:\n{source}"
    );
}

#[test]
fn untyped_receiver_runs_with_rails_answer() {
    app()
        .run_ruby(
            r##"
widget = Widget.create!(name: "alpha", extras: { "k" => "v" })
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "PATCH", "PATH_INFO" => "/widgets/#{widget.id}", "QUERY_STRING" => "ref=r-9", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
raise "status #{status} #{text}" unless status == 200
raise "body #{text.inspect}" unless text == "alpha,k,seen|v|r-9"
puts "ALL OK"
"##,
        )
        .assert_passes();
}
