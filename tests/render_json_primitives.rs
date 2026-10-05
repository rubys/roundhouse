//! Inline primitive JSON must use an encoder present in the compiled tree.
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

/// Build generic controllers covering literal and conditional primitive payloads.
fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\n  get \"/payload\", to: \"payloads#show\"\n  get \"/list\", to: \"payloads#index\"\n  get \"/choose\", to: \"payloads#choose\"\nend\n")
        .write("app/controllers/payloads_controller.rb", r#"class PayloadsController < ApplicationController
  def show
    render json: { message: "hello\n\"world\"", html: "<b>&</b>", count: 2, active: true, missing: nil, nested: { tags: ["one", "two"], empty: [], object: {} } }, status: 202
  end
  def index
    render json: [{ name: "first", count: 1 }, { name: "second", count: 2 }]
  end
  def choose
    render json: (params[:shape] == "array" ? ["one"] : { name: "one" })
  end
end
"#)
}

const ASSERTIONS: &str = r#"
require_relative "app/controllers/payloads_controller"
controller = PayloadsController.new
controller.process_action(:show)
raise "wrong status" unless controller.status == 202
raise "wrong content type" unless controller.content_type == "application/json"
raise controller.body unless controller.body == '{"message":"hello\n\"world\"","html":"\u003cb\u003e\u0026\u003c/b\u003e","count":2,"active":true,"missing":null,"nested":{"tags":["one","two"],"empty":[],"object":{}}}'
controller = PayloadsController.new
controller.process_action(:index)
raise controller.body unless controller.body == '[{"name":"first","count":1},{"name":"second","count":2}]'
controller = PayloadsController.new
controller.params = {"shape" => "array"}
controller.process_action(:choose)
raise controller.body unless controller.body == '["one"]'
controller = PayloadsController.new
controller.params = {"shape" => "hash"}
controller.process_action(:choose)
raise controller.body unless controller.body == '{"name":"one"}'
puts "primitive JSON passed"
"#;

/// CRuby preserves primitive payload bytes, status, and content type.
#[test]
fn inline_primitive_json_runs() {
    app().run_ruby(ASSERTIONS).assert_passes();
}

/// Temporal values must retain Rails serialization instead of primitive encoding.
#[test]
fn a_nested_time_keeps_rails_json_serialization() {
    app()
        .write("app/controllers/payloads_controller.rb", r#"class PayloadsController < ApplicationController
  def show
    render json: { at: Time.utc(2026, 7, 1, 12, 34, 56) }
  end
  def index
    head :no_content
  end
  def choose
    head :no_content
  end
end
"#)
        .run_ruby(r#"
require_relative "app/controllers/payloads_controller"
controller = PayloadsController.new
controller.process_action(:show)
raise controller.body unless controller.body == '{"at":"2026-07-01T12:34:56.000Z"}'
"#)
        .assert_passes();
}

/// The compiled runtime handles the same primitive and conditional payloads.
#[test]
#[ignore = "requires the Spinel toolchain"]
fn inline_primitive_json_runs_on_spinel() {
    app().run_spinel(ASSERTIONS).assert_passes();
}

/// These targets flatten runtime constants into one namespace. JSON escaping
/// must coexist with ViewHelpers' HTML escaping when both runtimes are emitted.
#[test]
fn json_and_view_html_escape_constants_do_not_collide() {
    use roundhouse::analyze::Analyzer;
    use roundhouse::emit::{crystal, csharp, go, kotlin, swift};
    use std::collections::BTreeSet;
    use std::path::Path;

    let mut app = roundhouse::ingest::ingest_app(Path::new("fixtures/tiny-blog"))
        .expect("ingest tiny-blog");
    Analyzer::new(&app).analyze(&mut app);
    let mut collisions = Vec::new();
    for (target, files, json_path, view_path, declaration) in [
        ("Go", go::emit(&app), "app/v2/json_builder.go", "app/v2/view_helpers.go",
         "var "),
        ("C#", csharp::emit(&app), "app/runtime/JsonBuilder.cs", "app/runtime/ViewHelpers.cs",
         "public static partial class RuntimeConstants { public static readonly "),
        ("Crystal", crystal::emit(&app), "src/json_builder.cr", "src/view_helpers.cr",
         ""),
        ("Kotlin", kotlin::emit(&app), "src/main/kotlin/JsonBuilder.kt", "src/main/kotlin/ViewHelpers.kt",
         "val "),
        ("Swift", swift::emit(&app), "Sources/App/JsonBuilder.swift", "Sources/App/ViewHelpers.swift",
         "let "),
    ] {
        let names = |path: &str| -> BTreeSet<String> {
            let file = files.iter().find(|file| file.path == Path::new(path))
                .unwrap_or_else(|| panic!("missing {target} runtime {path}"));
            let names: BTreeSet<_> = file.content.lines()
                .filter_map(|line| line.strip_prefix(declaration))
                .filter_map(|line| line.split_once(" ="))
                .filter_map(|(left, _)| left.split_whitespace().last())
                .filter(|name| name.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_'))
                .map(str::to_string).collect();
            assert!(!names.is_empty(), "no {target} runtime constants found in {path}");
            names
        };
        let json_names = names(json_path);
        let view_names = names(view_path);
        for name in json_names.intersection(&view_names) {
            collisions.push(format!("{target}: {name} is declared in both {json_path} and {view_path}"));
        }
    }
    assert!(collisions.is_empty(), "{}", collisions.join("\n"));
}
