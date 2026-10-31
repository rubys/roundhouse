//! A zero-arg `iso8601` on a jbuilder view's ivar prints as `iso8601` on
//! the Ruby family.
//!
//! `time_calendar` stores a time's zero-arg `iso8601` under Crystal's
//! name, `to_rfc3339`, and marks the send `GROUNDED_TIME_FORMAT` so the
//! Ruby emitter prints `iso8601` again even when it cannot type the
//! receiver. The jbuilder lowering turns `@since` into the view's `since`
//! parameter, which the emitter sees untyped, and rebuilt the send on the
//! way (`rewrite_ivars_to_locals`, `rewrite_route_helpers`) without that
//! mark, so the emitted Ruby called `since.to_rfc3339`: a NoMethodError
//! on MRI's Time, and on Spinel's.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n",
        )
        .write("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::API\nend\n")
        .write(
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    @since = Time.now.utc.beginning_of_month\n  end\nend\n",
        )
        .write("app/views/widgets/show.json.jbuilder", "json.since(@since.iso8601)\njson.name(@widget.name)\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :widgets, only: %i[show]\nend\n")
}

#[test]
fn view_param_iso8601_emits_iso8601() {
    use roundhouse::project::BuildTarget;
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let (tree, errors) = app().emit(target);
        assert!(errors.is_empty(), "{target:?}: {errors:?}");
        let view = std::fs::read_to_string(tree.join("app/views/widgets/show_json.rb")).expect("view emitted");
        assert!(
            view.contains("since.iso8601") && !view.contains("to_rfc3339"),
            "{target:?} must print `iso8601`:\n{view}"
        );
    }
}

#[test]
fn view_param_iso8601_runs() {
    let run = app().run_ruby(
        r##"
w = Widget.create!(name: "widget")
status, _headers, body = Main.run_rack("REQUEST_METHOD" => "GET", "PATH_INFO" => "/widgets/#{w.id}.json", "QUERY_STRING" => "", "rack.input" => StringIO.new(""))
text = +""
body.each { |part| text << part }
since = Time.now.utc.strftime("%Y-%m-01T00:00:00Z")
raise "show #{[status, text].inspect}" unless [status, text] == [200, %({"since":"#{since}","name":"widget"})]
puts "ALL OK"
"##,
    );
    run.assert_passes();
}
