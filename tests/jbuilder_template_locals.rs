//! A template local: `x = <expr>` in a jbuilder template, read by the
//! statements after it.
//!
//! The assignment was an Unknown statement, so it became an empty
//! append. Next to a whole-template `json.array!` or `json.partial!` it
//! also made the template two statements long, so that form went down
//! the object path, where a whole-template form is dropped, and the
//! template rendered `{}`. In an object template the assignment was dropped and
//! the pair that reads it kept: a `NameError` when the template runs.
//!
//! Two layers: the emitted Ruby, and the templates rendered on CRuby
//! (the emitted view modules plus the runtime's `JsonBuilder`, with
//! plain Structs for records) against what Rails 8.1 + jbuilder 2.15
//! render for the same templates and rows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
    t.integer "size"
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  get "widget_listed", to: "widgets#listed", defaults: { format: :json }
  get "widget_summary", to: "widgets#summary", defaults: { format: :json }
  get "widget_picked", to: "widgets#picked", defaults: { format: :json }
  get "widget_linked", to: "widgets#linked", defaults: { format: :json }
  resources :widgets, only: :show
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def listed
    @widgets = Widget.all
    render :listed
  end

  def summary
    @widgets = Widget.all
    render :summary
  end

  def picked
    @widgets = Widget.all
    render :picked
  end

  def linked
    @widgets = Widget.all
    render :linked
  end

  def show
    @widget = Widget.find(params[:id])
  end
end
"#;

const PARTIAL: &str = "json.id widget.id\njson.name widget.name\n";

/// A local, then a whole-template `array!` over it.
const LISTED: &str = r#"rows = @widgets.to_a
json.array! rows, partial: "widgets/widget", as: :widget
"#;

/// A local read by two pairs.
const SUMMARY: &str = r#"count = @widgets.size
json.count count
json.empty count.zero?
"#;

/// A local, then a whole-template `partial!` that passes it.
const PICKED: &str = r#"first = @widgets.first
json.partial! "widgets/widget", widget: first
"#;

/// A local whose value is a route helper call.
const LINKED: &str = r#"first = @widgets.first
link = widget_url(first)
json.href link
"#;

fn emitted() -> Vec<(String, String)> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("app/views/widgets/_widget.json.jbuilder", PARTIAL),
        ("app/views/widgets/listed.json.jbuilder", LISTED),
        ("app/views/widgets/summary.json.jbuilder", SUMMARY),
        ("app/views/widgets/picked.json.jbuilder", PICKED),
        ("app/views/widgets/linked.json.jbuilder", LINKED),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    ruby::emit_lowered_jbuilder_views(&app)
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect()
}

fn view<'a>(files: &'a [(String, String)], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| {
            panic!(
                "no emitted view ends with {suffix}; got {:?}",
                files.iter().map(|(p, _)| p).collect::<Vec<_>>()
            )
        })
}

#[test]
fn every_emitted_view_parses() {
    for (path, source) in emitted().iter().filter(|(p, _)| p.ends_with(".rb")) {
        let result = ruby_prism::parse(source.as_bytes());
        let errors: Vec<String> = result.errors().map(|e| e.message().to_string()).collect();
        assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
    }
}

#[test]
fn the_local_is_kept_before_the_statements_that_read_it() {
    let files = emitted();
    let src = view(&files, "widgets/listed_json.rb");
    let assign = src.find("rows = widgets.to_a").unwrap_or_else(|| panic!("the local:\n{src}"));
    let array = src
        .find("rows.map { |widget| Views::Widgets.widget_json(widget) }")
        .unwrap_or_else(|| panic!("the whole-template array over it:\n{src}"));
    assert!(assign < array, "the local comes first:\n{src}");
    let src = view(&files, "widgets/summary_json.rb");
    let assign = src.find("count = widgets.size").unwrap_or_else(|| panic!("the local:\n{src}"));
    let pair = src.find("\\\"count\\\":").unwrap_or_else(|| panic!("the pair:\n{src}"));
    assert!(assign < pair, "the local comes first:\n{src}");
    let src = view(&files, "widgets/picked_json.rb");
    let assign = src.find("first = widgets.first").unwrap_or_else(|| panic!("the local:\n{src}"));
    let call = src
        .find("io << Views::Widgets.widget_json(first)")
        .unwrap_or_else(|| panic!("the whole-template partial call with it:\n{src}"));
    assert!(assign < call, "the local comes first:\n{src}");
}

/// A local's value gets the rewrites a pair's value gets: the emitted
/// view has `RouteHelpers.<x>_path`, not `<x>_url`. A value with no
/// helper in it (`summary`'s `widgets.size`) is kept as written.
#[test]
fn a_route_helper_in_a_local_is_rewritten() {
    let files = emitted();
    let src = view(&files, "widgets/linked_json.rb");
    assert!(
        src.contains("link = RouteHelpers.widget_path(first.id)"),
        "the route helper is the runtime's path helper:\n{src}"
    );
    assert!(!src.contains("widget_url"), "no `_url` helper is left:\n{src}");
}

/// Render the templates on CRuby and compare with what Rails 8.1.4 +
/// jbuilder 2.15.1 answer for the same rows (`b`, `a`, `c`).
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-template-locals-{}",
        std::process::id()
    ));
    // The emitted tree as it is laid out. `app/views.rb` is the views
    // index a partial call requires; it holds nothing these templates
    // need.
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let mut requires = String::new();
    for name in ["_widget_json.rb", "listed_json.rb", "summary_json.rb", "picked_json.rb"] {
        let file = dir.join("app/views/widgets").join(name);
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r#"require {runtime:?}
{requires}
require "json"
Widget = Struct.new(:id, :name, :size)
widgets = [Widget.new(1, "b", 5), Widget.new(2, "a", nil), Widget.new(3, "c", nil)]
puts JSON.generate(
  "listed" => JSON.parse(Views::Widgets.listed_json(widgets)),
  "summary" => JSON.parse(Views::Widgets.summary_json(widgets)),
  "picked" => JSON.parse(Views::Widgets.picked_json(widgets)),
)
"#,
        runtime = runtime.display().to_string()
    );
    let output = Command::new("ruby").args(["-e", &script]).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Each value is what the Rails app answers for that template.
    let expected = concat!(
        r#"{"listed":[{"id":1,"name":"b"},{"id":2,"name":"a"},{"id":3,"name":"c"}],"#,
        r#""summary":{"count":3,"empty":false},"#,
        r#""picked":{"id":1,"name":"b"}}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
