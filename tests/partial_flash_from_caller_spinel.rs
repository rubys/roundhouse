//! The native side of `tests/partial_flash_from_caller.rs`: a partial
//! reads its caller's flash on Spinel too, where every template's flash
//! param is a typed `String | nil` the caller now passes along — from a
//! view, a layout, a collection, another partial, and an action's
//! `render partial:` — and neither a `local_assigns[:notice_]`, an outer
//! partial's `notice:` local nor a `|notice|` block param passes for it.
//! A `local_assigns[:notice]` beside it reads only the render's local.
//! The Spinel run needs a Spinel compiler, so it is
//! `#[ignore]`d like its sibling suites; CI's `spinel-framework` job runs
//! it with `--ignored` (`scripts/ci-plan.py`'s `SPINEL_TESTS`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const CONTROLLER: &str = r#"class PagesController < ApplicationController
  def show
    flash.now[:notice] = "Saved"
    flash.now[:alert] = "Oops"
    @rows = ["a", "b"]
  end

  def note
    flash.now[:notice] = "Rendered now"
    render partial: "note"
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write("app/controllers/pages_controller.rb", CONTROLLER)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"/page\", to: \"pages#show\"\n  get \"/note\", to: \"pages#note\"\nend\n",
        )
        .write(
            "app/views/layouts/application.html.erb",
            "<html><body>\n<%= render \"shared/flash\" %>\n<%= yield %>\n</body></html>\n",
        )
        .write("app/views/shared/_flash.html.erb", "<p id=\"layout\"><%= notice %>/<%= alert %></p>\n")
        .write(
            "app/views/pages/show.html.erb",
            "<%= render \"note\" %>\n<%= render partial: \"row\", collection: @rows, as: :row %>\n<%= render \"outer\" %>\n\
             <%= render \"opt\" %>\n<%= render partial: \"local_outer\", locals: { notice: \"Local\" } %>\n\
             <%= render \"strict_outer\", label: \"S:\" %>\n\
             <% [\"Local\"].each do |notice| %><%= render \"inner\" %><% end %>\n\
             <%= render \"local\" %>\n<%= render \"local\", notice: \"Local\" %>\n",
        )
        .write("app/views/pages/_note.html.erb", "<p id=\"note\"><%= notice %></p>\n")
        .write("app/views/pages/_row.html.erb", "<p class=\"row\"><%= row %>:<%= alert %></p>\n")
        .write("app/views/pages/_outer.html.erb", "<div id=\"outer\"><%= render \"inner\" %></div>\n")
        .write("app/views/pages/_inner.html.erb", "<p id=\"inner\"><%= notice %></p>\n")
        .write("app/views/pages/_opt.html.erb", "<p id=\"opt\"><%= notice %>|<%= local_assigns[:notice_] %></p>\n")
        .write(
            "app/views/pages/_local.html.erb",
            "<p class=\"local\"><%= notice %>|<%= local_assigns[:notice] || \"none\" %></p>\n",
        )
        .write(
            "app/views/pages/_local_outer.html.erb",
            "<div id=\"local-outer\"><%= notice %><%= render \"inner\" %></div>\n",
        )
        .write(
            "app/views/pages/_strict_outer.html.erb",
            "<%# locals: (label:, notice: \"Declared\") -%>\n\
             <div id=\"strict-outer\"><%= label %><%= notice %><%= render \"inner\" %></div>\n",
        )
}

const ASSERTIONS: &str = r#"
require_relative "app/controllers/pages_controller"

page = PagesController.new
page.params = {}
page.process_action(:show)
body = page.body
[
  "<p id=\"layout\">Saved/Oops</p>",
  "<p id=\"note\">Saved</p>",
  "<p class=\"row\">a:Oops</p>",
  "<p class=\"row\">b:Oops</p>",
  "<div id=\"outer\"><p id=\"inner\">Saved</p>",
  "<p id=\"opt\">Saved|</p>",
  "<div id=\"local-outer\">Local<p id=\"inner\">",
  "<div id=\"strict-outer\">S:Declared<p id=\"inner\">",
  "<p class=\"local\">Saved|none</p>",
  "<p class=\"local\">Local|Local</p>",
].each do |want|
  raise "show: no #{want} in #{body}" unless body.include?(want)
end
["Local", "Declared"].each do |local|
  raise "show: the inner partial read the outer's #{local}: #{body}" if body.include?("<p id=\"inner\">#{local}")
end

note = PagesController.new
note.params = {}
note.process_action(:note)
raise "note: #{note.body}" unless note.body.include?("<p id=\"note\">Rendered now</p>")
puts "every partial read its caller's flash"
"#;

#[test]
fn a_partial_reads_its_callers_flash_in_ruby() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("every partial read its caller's flash"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_partial_reads_its_callers_flash_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("every partial read its caller's flash"), "{}", run.stdout);
}
