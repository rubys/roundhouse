#[path = "support/emit_and_run.rs"]
#[allow(dead_code)]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\nend\n",
        )
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\n  resources :items, only: :create\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::API\nend\n",
        )
        .write(
            "app/controllers/items_controller.rb",
            r#"class ItemsController < ApplicationController
  def create
    @calls = 0
    first = item_params
    second = item_params
    render json: { calls: @calls, first: first.to_h, second: second.to_h }
  end

  private

  def item_params
    @item_params ||= begin
      @calls += 1
      params.expect(item: [:name, { labels: [] }])
    end
  end
end
"#,
        )
}

#[test]
fn memoized_params_are_filtered_once_and_return_the_cached_record() {
    app().run_ruby(r##"require "json"
require "stringio"
input = JSON.generate("item" => { "name" => "kept", "ignored" => "removed" })
status, _, body = Main.run_rack("REQUEST_METHOD" => "POST", "PATH_INFO" => "/items", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => input.bytesize.to_s, "rack.input" => StringIO.new(input))
expected = {"calls" => 1, "first" => {"name" => "kept"}, "second" => {"name" => "kept"}}
raise "#{status} #{body.inspect}" unless status == 200 && JSON.parse(body.join) == expected
puts "PASS"
"##).assert_passes();
}

#[test]
fn memoized_helpers_use_a_generated_params_factory() {
    let (emitted, errors) = app().emit(roundhouse::project::BuildTarget::Spinel);
    assert!(errors.is_empty(), "{errors:?}");
    let source = std::fs::read_to_string(emitted.join("app/controllers/items_controller.rb"))
        .expect("controller");
    assert!(source.contains("ItemParams.from_raw("), "{source}");
    assert!(!source.contains("@params.require("), "{source}");
    assert!(!source.contains("@params.expect("), "{source}");
}

#[test]
fn memoized_require_permit_and_local_filters_run_in_conditional_branches() {
    for filter in [
        "params.require(:item).permit(:name)",
        "values = params.expect(item: [:name]); values",
    ] {
        let source = format!(r#"class ItemsController < ApplicationController
  def create
    render json: item_params.to_h
  end

  private

  def item_params
    @item_params ||= begin
      if params[:mode] == "first"
        {filter}
      else
        params.expect(item: [:name])
      end
    end
  end
end
"#);
        app().write("app/controllers/items_controller.rb", &source).run_ruby(r##"require "json"
require_relative "app/controllers/items_controller"
["first", "second"].each do |mode|
  controller = ItemsController.new
  controller.params = {"mode" => mode, "item" => {"name" => "kept", "ignored" => "removed"}}
  controller.create
  raise controller.body unless JSON.parse(controller.body) == {"name" => "kept"}
end
puts "PASS"
"##).assert_passes();
    }
}

#[test]
fn memoized_upload_filters_preserve_files_through_helpers_and_locals() {
    app().write("app/controllers/items_controller.rb", r#"class ItemsController < ApplicationController
  def create
    values = item_params
    copied = values
    render json: upload_data(copied[:attachments])
  end

  private

  def item_params
    @item_params ||= begin
      if params[:mode] == "first"
        values = params.expect(item: [:name, {attachments: []}])
        values
      else
        params.expect(item: [:name, {attachments: []}])
      end
    end
  end

  def upload_data(uploads)
    Array(uploads).map do |file|
      {name: file.original_filename, type: file.content_type, data: file.read}
    end
  end
end
"#).run_ruby(r##"require "json"
require_relative "app/controllers/items_controller"
["first", "second"].each do |mode|
  controller = ItemsController.new
  file = ActionDispatch::Http::UploadedFile.new("bytes", "one.txt", "text/plain")
  controller.params = {"mode" => mode, "item" => {"name" => "kept", "attachments" => [file]}}
  controller.create
  expected = [{"name" => "one.txt", "type" => "text/plain", "data" => "bytes"}]
  raise controller.body unless JSON.parse(controller.body) == expected
end
puts "PASS"
"##).assert_passes();
}

fn model_app(helper: &str) -> emit_and_run::Overlay {
    app()
        .write(
            "db/schema.rb",
            "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table :items do |t|\n    t.string :name\n  end\nend\n",
        )
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write("app/models/item.rb", "class Item < ApplicationRecord\nend\n")
        .write("app/controllers/items_controller.rb", &format!(r#"class ItemsController < ApplicationController
  def create
    @mode = params[:mode]
    item = Item.new(item_params)
    render json: {{ name: item.name }}
  end

  private

{helper}
end
"#))
}

fn create_item(names: &[(&str, &str)]) -> String {
    let names = names.iter().map(|(mode, name)| format!("{mode:?} => {name:?}")).collect::<Vec<_>>().join(", ");
    format!(r##"require "json"
require "stringio"
{{ {names} }}.each do |mode, name|
  input = JSON.generate("mode" => mode, "item" => {{ "name" => "provided", "ignored" => "removed" }})
  status, _, body = Main.run_rack("REQUEST_METHOD" => "POST", "PATH_INFO" => "/items", "QUERY_STRING" => "", "CONTENT_TYPE" => "application/json", "CONTENT_LENGTH" => input.bytesize.to_s, "rack.input" => StringIO.new(input))
  raise "#{{mode}}: #{{status}} #{{body.inspect}}" unless status == 200 && JSON.parse(body.join) == {{ "name" => name }}
end
puts "PASS"
"##)
}

fn emitted_controller(emitted: &std::path::Path) -> String {
    std::fs::read_to_string(emitted.join("app/controllers/items_controller.rb")).expect("controller")
}

#[test]
fn helpers_returning_permit_filters_construct_models_through_the_params_factory() {
    for helper in [
        r#"  def item_params
    @item_params ||= if params[:mode] == "first"
      params.require(:item).permit(:name)
    else
      values = params.expect(item: [:name])
      values
    end
  end"#,
        r#"  def item_params
    return params.require(:item).permit(:name) if params[:mode] == "first"
    @item_params ||= params.expect(item: [:name])
  end"#,
    ] {
        let run = model_app(helper).run_ruby(&create_item(&[("first", "provided"), ("second", "provided")]));
        run.assert_passes();
        let controller = emitted_controller(&run.emitted);
        assert!(controller.contains("Item.from_params(self.item_params)"), "{controller}");
    }
}

#[test]
fn filters_outside_returned_values_leave_hash_helpers_on_the_model_constructor() {
    for helper in [
        r#"  def item_params
    case params[:mode]
    when "raw"
      { name: "kept" }
    when params.expect(item: [:name])
      { name: "matched" }
    else
      { name: "other" }
    end
  end"#,
        r#"  def item_params
    case @mode
    in "raw"
      { name: "kept" }
    in ^(params.expect(item: [:name]))
      { name: "matched" }
    else
      { name: "other" }
    end
  end"#,
        r#"  def item_params
    case
    when params[:mode] == "raw"
      { name: "kept" }
    when params.expect(item: [:name])
      { name: "matched" }
    else
      { name: "other" }
    end
  end"#,
        r#"  def item_params
    begin
      { name: "kept" }
    rescue (params.expect(item: [:name]))
      { name: "rescued" }
    end
  end"#,
        r#"  def item_params
    unused = ->(value = params.expect(item: [:name])) { value }
    { name: "kept" }
  end"#,
        r#"  def item_params
    while params[:mode] != "raw" && params.expect(item: [:name])
      break
    end
    { name: "kept" }
  end"#,
        r#"  def item_params
    begin
      { name: "kept" }
    ensure
      unused = -> { params.expect(item: [:name]) }
    end
  end"#,
    ] {
        let run = model_app(helper).run_ruby(&create_item(&[("raw", "kept")]));
        run.assert_passes();
        let controller = emitted_controller(&run.emitted);
        assert!(controller.contains("Item.new(self.item_params)"), "{controller}");
    }
}

#[test]
fn helpers_returning_a_hash_on_one_path_and_a_filter_on_another_keep_the_model_constructor() {
    for helper in [
        r#"  def item_params
    case params[:mode]
    when "raw"
      { name: "kept" }
    else
      params.require(:item).permit(:name)
    end
  end"#,
        r#"  def item_params
    case params[:mode]
    when "raw"
      values = { name: "kept" }
    else
      values = params.require(:item).permit(:name)
    end
    values
  end"#,
        r#"  def item_params
    if params[:mode] == "raw"
      { name: "kept" }
    else
      params.expect(item: [:name])
    end
  end"#,
        r#"  def item_params
    if params[:mode] == "raw"
      values = { name: "kept" }
    else
      values = params.expect(item: [:name])
    end
    values
  end"#,
    ] {
        let run = model_app(helper).run_ruby(&create_item(&[("raw", "kept"), ("filtered", "provided")]));
        run.assert_passes();
        let controller = emitted_controller(&run.emitted);
        assert!(controller.contains("Item.new(self.item_params)"), "{controller}");
    }
}

#[test]
fn filters_returned_through_other_calls_keep_the_model_constructor() {
    for helper in [
        r#"  def item_params
    params.require(:item).permit(:name).tap { |values| values }
  end"#,
        r#"  def item_params
    same(params.require(:item).permit(:name))
  end

  def same(values)
    values
  end"#,
        r#"  def item_params
    [1].map { params.require(:item).permit(:name) }.first
  end"#,
        r#"  def item_params
    { name: params.require(:item).permit(:name)[:name] }
  end"#,
    ] {
        let run = model_app(helper).run_ruby(&create_item(&[("any", "provided")]));
        run.assert_passes();
        let controller = emitted_controller(&run.emitted);
        assert!(controller.contains("Item.new(self.item_params)"), "{controller}");
    }
}

#[test]
fn filters_in_case_patterns_and_default_arguments_leave_the_model_constructor() {
    for helper in [
        r#"  def item_params
    case params[:mode]
    in "raw"
      { name: "kept" }
    in ^(params.expect(item: [:name]))
      { name: "matched" }
    else
      { name: "other" }
    end
  end"#,
        r#"  def item_params(unused = params.expect(item: [:name]))
    { name: "kept" }
  end"#,
    ] {
        let (emitted, errors) = model_app(helper).emit(roundhouse::project::BuildTarget::Ruby);
        assert!(errors.is_empty(), "{errors:?}");
        let controller = emitted_controller(&emitted);
        assert!(controller.contains("Item.new(self.item_params)"), "{controller}");
    }
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn memoized_params_run_with_emitted_rbs() {
    app().run_spinel_with_rbs(r##"require "json"
require_relative "app/controllers/items_controller"
controller = ItemsController.new
controller.params = {"item" => {"name" => "kept", "ignored" => "removed"}}
controller.create
expected = {"calls" => 1, "first" => {"name" => "kept"}, "second" => {"name" => "kept"}}
raise controller.body unless JSON.parse(controller.body) == expected
puts "PASS"
"##).assert_passes();
}
