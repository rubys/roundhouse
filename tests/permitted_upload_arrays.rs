#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("db/schema.rb", "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :prompts, only: :create\nend\n")
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("app/controllers/prompts_controller.rb", r#"class PromptsController < ApplicationController
  def create
    parameters = prompt_params
    render json: { uploads: upload_data(parameters[:attachments]), tags: parameters[:tags] }
  end

  private
  def prompt_params
    params.expect(prompt: [:body, { attachments: [], tags: [] }])
  end

  def upload_data(uploads)
    Array(uploads).map do |file|
      { name: file.original_filename, type: file.content_type, data: file.read }
    end
  end
end
"#)
}

const CONTRACT: &str = r##"require "json"
require_relative "app/controllers/prompts_controller"
one = ActionDispatch::Http::UploadedFile.new("first", "one.txt", "text/plain")
two = ActionDispatch::Http::UploadedFile.new("second", "two.txt", "text/plain")
controller = PromptsController.new
controller.params = {"prompt" => {"body" => "hello", "attachments" => [one, two], "tags" => ["a", "b"]}}
controller.create
expected = {"uploads" => [{"name" => "one.txt", "type" => "text/plain", "data" => "first"}, {"name" => "two.txt", "type" => "text/plain", "data" => "second"}], "tags" => ["a", "b"]}
raise controller.body unless JSON.parse(controller.body) == expected
empty = PromptsController.new
empty.params = {"prompt" => {"body" => "hello"}}
empty.create
raise empty.body unless JSON.parse(empty.body) == {"uploads" => [], "tags" => []}
bad = PromptsController.new
bad.params = {"prompt" => {"body" => "hello", "attachments" => ["not a file"]}}
begin
  bad.create
  raise "accepted a String as an upload object"
rescue NoMethodError
end
puts "PASS"
"##;

#[test]
fn permitted_uploads_keep_objects_and_metadata() {
    app().run_ruby(CONTRACT).assert_passes();
}

#[test]
fn directly_assigned_params_keep_uploads_in_map_blocks() {
    app().write("app/controllers/prompts_controller.rb", r#"class PromptsController < ApplicationController
  def create
    values = params.expect(prompt: [:body, {attachments: [], tags: []}])
    uploads = values[:attachments].map do |file|
      {name: file.original_filename, type: file.content_type, data: file.read}
    end
    render json: {uploads: uploads, tags: values[:tags]}
  end
end
"#).run_ruby(CONTRACT).assert_passes();
}

#[test]
fn inline_params_keep_uploads_in_each_blocks() {
    app().write("app/controllers/prompts_controller.rb", r#"class PromptsController < ApplicationController
  def create
    uploads = []
    params.expect(prompt: [:body, {attachments: []}])[:attachments].each do |file|
      uploads.push({name: file.original_filename, type: file.content_type, data: file.read})
    end
    render json: {uploads: uploads, tags: params.expect(prompt: [:body, {tags: []}])[:tags]}
  end
end
"#).run_ruby(CONTRACT).assert_passes();
}

#[test]
fn unrecognized_params_aliases_keep_upload_method_errors() {
    for bindings in [
        "values = prompt_params\n    copied = values",
        "copied = prompt_params\n    copied = prompt_params",
    ] {
        let errors = app().write("app/controllers/prompts_controller.rb", &format!(r#"class PromptsController < ApplicationController
  def create
    {bindings}
    render json: copied[:attachments].map {{ |file| file.read }}
  end
  private
  def prompt_params
    params.expect(prompt: [:body, {{attachments: []}}])
  end
end
"#)).check_errors();
        assert!(errors.iter().any(|error| error.contains("read") && error.contains("String")), "{bindings}: {errors:?}");
    }
}

#[test]
fn conditional_params_with_distinct_filters_keep_upload_method_errors() {
    for bindings in ["values = prompt_params", r#"values = if params[:mode] == "first"
      params.expect(prompt: [:body, {attachments: []}])
    else
      params.expect(prompt: [:title, {attachments: []}])
    end"#] {
        let errors = app().write("app/controllers/prompts_controller.rb", &format!(r#"class PromptsController < ApplicationController
  def create
    {bindings}
    render json: Array(values[:attachments]).map {{ |file| {{name: file.original_filename, type: file.content_type, data: file.read}} }}
  end
  private
  def prompt_params
    if params[:mode] == "first"
      params.expect(prompt: [:body, {{attachments: []}}])
    else
      params.expect(prompt: [:title, {{attachments: []}}])
    end
  end
end
"#)).check_errors();
        for method in ["original_filename", "content_type", "read"] {
            assert!(errors.iter().any(|error| error.contains(method) && error.contains("String")), "{bindings}: {errors:?}");
        }
    }
}

#[test]
fn conditional_params_with_the_same_filter_keep_upload_objects() {
    app().write("app/controllers/prompts_controller.rb", r#"class PromptsController < ApplicationController
  def create
    values = prompt_params
    render json: {uploads: upload_data(values[:attachments]), tags: values[:tags]}
  end
  private
  def prompt_params
    if params[:body]
      params.expect(prompt: [:body, {attachments: [], tags: []}])
    else
      params.expect(prompt: [:body, {attachments: [], tags: []}])
    end
  end
  def upload_data(uploads)
    Array(uploads).map do |file|
      {name: file.original_filename, type: file.content_type, data: file.read}
    end
  end
end
"#).run_ruby(CONTRACT).assert_passes();
}

#[test]
fn ordinary_array_fields_keep_string_methods_in_source_analysis() {
    let errors = app().write("app/controllers/prompts_controller.rb", r#"class PromptsController < ApplicationController
  def create
    values = params.expect(prompt: [:body, {tags: []}])
    render json: values[:tags].map(&:upcase)
  end
end
"#).check_errors();
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn upload_array_and_ordinary_scalar_array_have_separate_readers() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let record = std::fs::read_to_string(emitted.join("app/models/prompt_params.rb")).unwrap();
    assert!(record.contains("UploadedFile.scalar_list("), "{record}");
    assert!(record.contains("Params.scalar_list("), "{record}");
    let rbs = std::fs::read_to_string(emitted.join("app/models/prompt_params.rbs")).unwrap();
    assert!(rbs.contains("UploadedFile"), "{rbs}");
    assert!(rbs.contains("attr_reader tags: Array[String]"), "{rbs}");
    let controller_rbs = std::fs::read_to_string(emitted.join("app/controllers/prompts_controller.rbs")).unwrap();
    let helper = controller_rbs.lines().find(|line| line.contains("def upload_data:")).unwrap();
    assert!(helper.contains("Array[") && helper.contains("UploadedFile"), "{helper}");
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn permitted_upload_arrays_run_with_emitted_rbs() {
    app().run_spinel_with_rbs(CONTRACT).assert_passes();
}
