//! The Spinel side of `tests/emit_and_run/helper_method_session.rs`'s
//! memoizing helper: a `helper_method` that keeps instance state runs on
//! the controller handling the request, and one pure through the methods
//! it calls is cloned class-side together with them. Needs a Spinel
//! compiler, so it is `#[ignore]`d like its sibling suites; CI's
//! `spinel-framework` job runs it with `--ignored` (`scripts/ci-plan.py`'s
//! `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"visits\" do |t|\n    t.string \"who\"\n  end\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/visitors/:who\", to: \"visitors#show\"\nend\n",
        )
        .write(
            "app/models/current.rb",
            "class Current < ActiveSupport::CurrentAttributes\n  attribute :visitor\nend\n",
        )
        .write(
            "app/controllers/visitors_controller.rb",
            r##"class VisitorsController < ApplicationController
  before_action :remember_visitor
  helper_method :visitor, :visitor_label, :greeting

  def show
  end

  def visitor
    @visitor ||= Current.visitor
  end

  def visitor_label
    "visitor #{visitor}"
  end

  def greeting(name)
    "#{salutation}, #{name}"
  end

  private

  def remember_visitor
    Current.visitor = params["who"].to_s
  end

  def salutation
    "hello"
  end
end
"##,
        )
        .write(
            "app/views/visitors/show.html.erb",
            "<p><%= visitor %>|<%= visitor_label %>|<%= greeting(\"ada\") %></p>\n",
        )
}

const ASSERTIONS: &str = r#"
first = VisitorsController.new
ActionController::Current.controller = first
first.params = { "who" => "ada" }
first.process_action(:show)
raise "first: body #{first.body.inspect}" unless first.body.include?("ada|visitor ada|hello, ada")

second = VisitorsController.new
ActionController::Current.controller = second
second.params = { "who" => "bob" }
second.process_action(:show)
raise "second: body #{second.body.inspect}" unless second.body.include?("bob|visitor bob|hello, ada")
puts "each request's helper answered from its own controller"
"#;

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_memoizing_helper_method_answers_each_request_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(
        run.stdout.contains("each request's helper answered from its own controller"),
        "{}",
        run.stdout
    );
}
