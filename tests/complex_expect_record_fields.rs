#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :entries, only: :create\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/entries_controller.rb", r#"class EntriesController < ApplicationController
  def create
    values = params.expect(entry: [:body, { tags: [], records: [[:name, :bytes]] }])
    render json: { records: values[:records], tags: values[:tags].map(&:to_s) }
  end
  private
  def entry_params
    params.expect(entry: [:body, { tags: [], records: [[:name, :bytes]] }])
  end
end
"#)
}

const CONTRACT: &str = r##"require "json"
require_relative "app/controllers/entries_controller"
controller = EntriesController.new
controller.params = {"entry" => {"body" => "hello", "tags" => ["one"], "records" => [{"name" => "notes.txt", "bytes" => 5, "ignored" => "removed"}]}}
controller.create
raise controller.body unless JSON.parse(controller.body) == {"tags" => ["one"], "records" => [{"name" => "notes.txt", "bytes" => 5}]}
numbered = EntriesController.new
numbered.params = {"entry" => {"body" => "hello", "tags" => [], "records" => {"0" => {"name" => "notes.txt", "bytes" => 5}, "ignored" => {"name" => "dropped"}}}}
numbered.create
raise numbered.body unless JSON.parse(numbered.body) == {"tags" => [], "records" => {"0" => {"name" => "notes.txt", "bytes" => 5}}}
empty = EntriesController.new
empty.params = {"entry" => {"body" => "hello", "tags" => []}}
empty.create
raise empty.body unless JSON.parse(empty.body) == {"tags" => [], "records" => nil}
puts "PASS"
"##;

#[test]
fn complex_expect_fields_keep_their_collection_contracts() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let rbs = std::fs::read_to_string(emitted.join("app/models/entry_params.rbs")).unwrap();
    assert!(rbs.contains("attr_reader tags: Array[String]"), "{rbs}");
    assert!(rbs.contains("Hash[String, Roundhouse::ParamValue]"), "{rbs}");
    assert!(!rbs.contains("attr_reader records: Roundhouse::ParamValue"), "{rbs}");
}

#[test]
fn complex_expect_records_keep_arrays_numbered_hashes_and_nil() {
    app().run_ruby(CONTRACT).assert_passes();
}

fn unsupported_filter_errors(filter: &str) -> Vec<String> {
    app().write("app/controllers/entries_controller.rb", &format!(r#"class EntriesController < ApplicationController
  def create
    values = params.expect({filter})
    render json: values[:tags].map(&:to_s)
  end
end
"#)).check_errors()
}

#[test]
fn string_resource_keys_keep_the_source_error() {
    let errors = unsupported_filter_errors(r#""entry" => [:body, {tags: []}]"#);
    assert!(errors.iter().any(|error| error.contains("map") && error.contains("String")), "{errors:?}");
}

#[test]
fn nonfinal_nested_hashes_keep_the_source_error() {
    for filter in ["entry: [{tags: []}, :body]", "entry: [{records: [[:name]]}, {tags: []}]"] {
        let errors = unsupported_filter_errors(filter);
        assert!(errors.iter().any(|error| error.contains("map") && error.contains("String")), "{filter}: {errors:?}");
    }
}

#[test]
fn unlowered_nested_fields_keep_the_source_error() {
    for filter in [r#"entry: [:body, {"tags" => []}]"#, "entry: [:body, {tags: [], records: [[]]}]", r#"entry: [:body, {tags: [], records: [["name"]]}]"#] {
        let errors = unsupported_filter_errors(filter);
        assert!(errors.iter().any(|error| error.contains("map") && error.contains("String")), "{filter}: {errors:?}");
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn complex_expect_fields_run_with_emitted_rbs() {
    app().run_spinel_with_rbs(CONTRACT).assert_passes();
}
