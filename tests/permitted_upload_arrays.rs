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

const BOTH_MODES: &str = r##"require "json"
require_relative "app/controllers/prompts_controller"
["first", "second"].each do |mode|
  controller = PromptsController.new
  controller.params = {"mode" => mode, "prompt" => {"body" => "hello", "title" => "hi", "attachments" => [ActionDispatch::Http::UploadedFile.new("bytes", "one.txt", "text/plain")]}}
  controller.create
  expected = [{"name" => "one.txt", "type" => "text/plain", "data" => "bytes"}]
  raise "#{mode}: #{controller.body}" unless JSON.parse(controller.body) == expected
end
puts "PASS"
"##;

fn prompts_controller(actions: &str, helpers: &str) -> String {
    format!(r#"class PromptsController < ApplicationController
{actions}
  def preview
    render json: Array(first_filter[:attachments]).map {{ |file| file.read }}
  end

  private
{helpers}
  def first_filter
    params.expect(prompt: [:body, {{attachments: []}}])
  end

  def second_filter
    params.expect(prompt: [:title, {{attachments: []}}])
  end
end
"#)
}

#[test]
fn every_permit_filter_keeps_uploads_of_a_field_read_as_uploads() {
    let create = r#"  def create
    values = prompt_params
    render json: Array(values[:attachments]).map { |file| {name: file.original_filename, type: file.content_type, data: file.read} }
  end
"#;
    let either = |second: &str| format!(r#"  def prompt_params
    if params[:mode] == "first"
      params.expect(prompt: [:body, {{attachments: []}}])
    else
      {second}
    end
  end
"#);
    for (case, actions, helpers) in [
        ("distinct filters", create.to_string(), either("params.expect(prompt: [:title, {attachments: []}])")),
        ("same filter", create.to_string(), either("params.expect(prompt: [:body, {attachments: []}])")),
        ("zero-argument fallback", create.to_string(), either("second_filter")),
        ("helper argument", r#"  def create
    values = prompt_params(params[:mode])
    render json: Array(values[:attachments]).map { |file| {name: file.original_filename, type: file.content_type, data: file.read} }
  end
"#.to_string(), r#"  def prompt_params(mode)
    mode == "first" ? first_filter : second_filter
  end
"#.to_string()),
        ("inline", r#"  def create
    values = if params[:mode] == "first"
      params.expect(prompt: [:body, {attachments: []}])
    else
      params.expect(prompt: [:title, {attachments: []}])
    end
    render json: Array(values[:attachments]).map { |file| {name: file.original_filename, type: file.content_type, data: file.read} }
  end
"#.to_string(), String::new()),
        ("alias", r#"  def create
    values = prompt_params
    copied = values
    render json: Array(copied[:attachments]).map { |file| {name: file.original_filename, type: file.content_type, data: file.read} }
  end
"#.to_string(), either("params.expect(prompt: [:title, {attachments: []}])")),
        ("first element", r#"  def create
    file = prompt_params[:attachments].first
    render json: [{name: file.original_filename, type: file.content_type, data: file.read}]
  end
"#.to_string(), either("params.expect(prompt: [:title, {attachments: []}])")),
    ] {
        let run = app()
            .write("config/routes.rb", "Rails.application.routes.draw do\n  resources :prompts, only: :create\n  post \"prompts/preview\", to: \"prompts#preview\"\nend\n")
            .write("app/controllers/prompts_controller.rb", &prompts_controller(&actions, &helpers))
            .run_ruby(BOTH_MODES);
        assert!(run.errors.is_empty() && run.success, "{case}: {:?}\n{}\n{}", run.errors, run.stdout, run.stderr);
    }
}

#[test]
fn string_reads_of_a_field_read_as_uploads_keep_strings() {
    let (emitted, errors) = strings_app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let fields: Vec<String> = std::fs::read_dir(emitted.join("app/models")).unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "rbs"))
        .flat_map(|path| std::fs::read_to_string(path).unwrap().lines().map(str::trim).map(String::from).collect::<Vec<_>>())
        .filter(|line| line.starts_with("attr_reader attachments:"))
        .collect();
    assert_eq!(fields, ["attr_reader attachments: Array[(String | ActionDispatch::Http::UploadedFile)]"; 2], "{fields:?}");
    strings_app().run_ruby(STRINGS).assert_passes();
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn string_reads_of_a_field_read_as_uploads_run_with_emitted_rbs() {
    strings_app().run_spinel_with_rbs(STRINGS).assert_passes();
}

fn strings_app() -> emit_and_run::Overlay {
    app().write("app/controllers/prompts_controller.rb", r#"class PromptsController < ApplicationController
  def create
    values = params.expect(prompt: [:title, {attachments: []}])
    render json: Array(values[:attachments]).map { |name| name.upcase }
  end

  def preview
    values = params.expect(prompt: [:body, {attachments: []}])
    render json: Array(values[:attachments]).map { |file| file.read }
  end
end
"#)
}

const STRINGS: &str = r##"require "json"
require_relative "app/controllers/prompts_controller"
controller = PromptsController.new
controller.params = {"prompt" => {"title" => "hi", "attachments" => ["abc", "def"]}}
controller.create
raise controller.body unless JSON.parse(controller.body) == ["ABC", "DEF"]
puts "PASS"
"##;

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
