//! A collection with a block: `json.<key> col do |x| … end` and
//! `json.array! col do |x| … end` (#323).
//!
//! Jbuilder's `set!` with a value AND a block is `array!` on the value
//! under that key: one object per element, each built by the block.
//! The classifier took the one-positional shape for a scalar pair, so
//! the block was dropped and the records went to
//! `JsonBuilder.encode_value`, which quoted their `inspect` as one
//! string. `array!` with a block and no `partial:` was Unknown, and the
//! template rendered `{}`.
//!
//! Two layers: the emitted Ruby for each template, and the templates
//! rendered on CRuby (the emitted view modules plus the runtime's
//! `JsonBuilder`, with plain Structs for records) against what Rails
//! 8.1 + jbuilder 2.15 render for the same templates and rows.

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
  get "widget_items", to: "widgets#items", defaults: { format: :json }
  get "widget_inline", to: "widgets#inline", defaults: { format: :json }
  get "widget_names", to: "widgets#names", defaults: { format: :json }
  get "widget_links", to: "widgets#links", defaults: { format: :json }
  resources :widgets, only: :show
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def items
    @widgets = Widget.all
    render :items
  end

  def inline
    @widgets = Widget.all
    render :inline
  end

  def names
    @widgets = Widget.all
    render :names
  end

  def links
    @widgets = Widget.all
    render :links
  end

  def show
    @widget = Widget.find(params[:id])
  end
end
"#;

const PARTIAL: &str = "json.id widget.id\njson.name widget.name\n";

/// The block is one partial call per element.
const ITEMS: &str =
    "json.widgets(@widgets) { |widget| json.partial!(\"widgets/widget\", widget: widget) }\n";

/// The block builds each element inline, after a pair of its own.
const INLINE: &str = r#"json.count @widgets.size
json.widgets @widgets do |widget|
  json.id widget.id
  json.label widget.name
end
"#;

/// `array!` with a block owns the whole template.
const NAMES: &str = "json.array!(@widgets) { |widget| json.name widget.name }\n";

/// The partial's argument is a route helper call.
const LINKS: &str =
    "json.links(@widgets) { |widget| json.partial!(\"widgets/link\", url: widget_url(widget)) }\n";

const LINK_PARTIAL: &str = "json.href url\n";

/// A partial next to another statement in the element. Jbuilder renders
/// both into the element; the lowerer cannot yet, so the pair is left
/// unsupported rather than built without the partial's fields.
const MIXED: &str = r#"json.flagged @widgets do |widget|
  json.partial! "widgets/widget", widget: widget
  json.flag true
end
"#;

/// A partial inside a nested object of the element. Jbuilder renders it
/// into that object; the lowerer cannot yet, the same as `MIXED`.
const NESTED_MIXED: &str = r#"json.boxed @widgets do |widget|
  json.box do
    json.partial! "widgets/widget", widget: widget
    json.flag true
  end
end
"#;

/// A nested object without a partial in the element still lowers, and
/// its collection is an expression, not a local.
const SORTED: &str = r#"json.sorted @widgets.sort_by(&:name) do |widget|
  json.id widget.id
  json.meta do
    json.size widget.size
  end
end
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
        ("app/views/widgets/items.json.jbuilder", ITEMS),
        ("app/views/widgets/inline.json.jbuilder", INLINE),
        ("app/views/widgets/names.json.jbuilder", NAMES),
        ("app/views/widgets/links.json.jbuilder", LINKS),
        ("app/views/widgets/_link.json.jbuilder", LINK_PARTIAL),
        ("app/views/widgets/mixed.json.jbuilder", MIXED),
        ("app/views/widgets/nested_mixed.json.jbuilder", NESTED_MIXED),
        ("app/views/widgets/sorted.json.jbuilder", SORTED),
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
fn a_key_with_a_collection_and_a_partial_block_is_an_array_of_partials() {
    let files = emitted();
    let src = view(&files, "widgets/items_json.rb");
    assert!(
        !src.contains("encode_value(widgets)"),
        "the collection is not a scalar value:\n{src}"
    );
    assert!(
        src.contains("__col0 = widgets")
            && src.contains("__col0.map { |widget| Views::Widgets.widget_json(widget) }.join(\",\")"),
        "one partial call per element:\n{src}"
    );
}

/// The partial's argument gets the rewrites any partial argument in a
/// pair gets: a `<x>_url` helper becomes `RouteHelpers.<x>_path`.
#[test]
fn the_partial_argument_of_a_block_element_is_rewritten() {
    let files = emitted();
    let src = view(&files, "widgets/links_json.rb");
    assert!(
        src.contains("Views::Widgets.link_json(RouteHelpers.widget_path(widget.id))"),
        "the route helper is the runtime's path helper:\n{src}"
    );
}

#[test]
fn a_key_with_a_collection_and_an_inline_block_builds_each_element() {
    let files = emitted();
    let src = view(&files, "widgets/inline_json.rb");
    assert!(
        !src.contains("encode_value(widgets)"),
        "the collection is not a scalar value:\n{src}"
    );
    assert!(
        src.contains("\\\"label\\\":") && src.contains("__col0 = widgets") && src.contains("__col0.map"),
        "the block's pairs are emitted per element:\n{src}"
    );
}

#[test]
fn array_bang_with_a_block_is_the_whole_template() {
    let files = emitted();
    let src = view(&files, "widgets/names_json.rb");
    assert!(
        src.contains("io << \"[\"") && src.contains("__col0 = widgets") && src.contains("__col0.map"),
        "a top-level array, one element per widget:\n{src}"
    );
    assert!(src.contains("\\\"name\\\":"), "the block's pair:\n{src}");
}

#[test]
fn a_partial_mixed_into_an_element_is_unsupported_not_dropped() {
    roundhouse::ingest::survey::activate();
    let files = emitted();
    let gaps = roundhouse::ingest::survey::drain();
    let src = view(&files, "widgets/mixed_json.rb");
    assert!(
        !src.contains("widgets.map") && !src.contains("\\\"flag\\\":"),
        "no element is built without the partial's fields:\n{src}"
    );
    assert!(
        gaps.iter().any(|g| g.to_string().contains("mixes `json.partial!` with other statements")),
        "the block is reported as unsupported: {:?}",
        gaps.iter().map(|g| g.to_string()).collect::<Vec<_>>()
    );
}

#[test]
fn a_partial_in_a_nested_object_of_an_element_is_unsupported_not_dropped() {
    roundhouse::ingest::survey::activate();
    let files = emitted();
    let gaps = roundhouse::ingest::survey::drain();
    let src = view(&files, "widgets/nested_mixed_json.rb");
    assert!(
        !src.contains(".map") && !src.contains("\\\"flag\\\":"),
        "no element is built without the partial's fields:\n{src}"
    );
    assert!(
        gaps.iter().any(|g| g.to_string().contains("mixes `json.partial!` with other statements")),
        "the block is reported as unsupported: {:?}",
        gaps.iter().map(|g| g.to_string()).collect::<Vec<_>>()
    );
}

/// The nil check and the `map` read one local, so the collection
/// expression runs once per render.
#[test]
fn the_collection_expression_is_evaluated_once() {
    let files = emitted();
    let src = view(&files, "widgets/sorted_json.rb");
    assert_eq!(src.matches("sort_by").count(), 1, "one evaluation of the collection:\n{src}");
    assert!(
        src.contains("__col0.nil?") && src.contains("__col0.map"),
        "the nil check and the map read the bound local:\n{src}"
    );
    assert!(src.contains("\\\"meta\\\":"), "a nested object without a partial still lowers:\n{src}");
}

/// The emitted views loaded on CRuby next to the runtime's
/// `JsonBuilder`, with `Widget` a Struct, then `body` run. Answers its
/// stdout.
fn render(name: &str, body: &str) -> String {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-collection-block-{name}-{}",
        std::process::id()
    ));
    // The emitted tree as it is laid out, partial first. `app/views.rb`
    // is the views index a partial call requires; it holds nothing
    // these templates need.
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let mut requires = String::new();
    for name in ["_widget_json.rb", "items_json.rb", "inline_json.rb", "names_json.rb", "sorted_json.rb"] {
        let file = dir.join("app/views/widgets").join(name);
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        "require {runtime:?}\n{requires}require \"json\"\nWidget = Struct.new(:id, :name, :size)\n{body}",
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
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Render the three templates on CRuby and compare with what Rails
/// 8.1.4 + jbuilder 2.15.1 answer for the same rows (`b`, `a`, `c`).
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let out = render(
        "rows",
        r#"widgets = [Widget.new(1, "b", 5), Widget.new(2, "a", nil), Widget.new(3, "c", nil)]
puts JSON.generate(
  "items" => JSON.parse(Views::Widgets.items_json(widgets)),
  "inline" => JSON.parse(Views::Widgets.inline_json(widgets)),
  "names" => JSON.parse(Views::Widgets.names_json(widgets)),
)
"#,
    );
    // Each value is what the Rails app answers for that template.
    let expected = concat!(
        r#"{"items":{"widgets":[{"id":1,"name":"b"},{"id":2,"name":"a"},{"id":3,"name":"c"}]},"#,
        r#""inline":{"count":3,"widgets":[{"id":1,"label":"b"},{"id":2,"label":"a"},{"id":3,"label":"c"}]},"#,
        r#""names":[{"name":"b"},{"name":"a"},{"name":"c"}]}"#,
    );
    assert_eq!(out, expected);
}

/// Jbuilder's `array!` answers `[]` for a nil collection, and a key
/// with a collection and a block goes through it: jbuilder 2.15.1
/// renders `json.array!(nil) { … }` as `[]` and `json.widgets(nil) { … }`
/// as `{"widgets":[]}`.
#[test]
fn a_nil_collection_renders_an_empty_array() {
    let out = render(
        "nil",
        r#"puts JSON.generate(
  "items" => JSON.parse(Views::Widgets.items_json(nil)),
  "names" => JSON.parse(Views::Widgets.names_json(nil)),
)
"#,
    );
    assert_eq!(out, r#"{"items":{"widgets":[]},"names":[]}"#);
}

/// The bound collection renders what jbuilder 2.15.1 renders for the
/// same template and rows, nested object included.
#[test]
fn an_expression_collection_renders_what_jbuilder_renders() {
    let out = render(
        "sorted",
        r#"widgets = [Widget.new(1, "b", 5), Widget.new(2, "a", nil), Widget.new(3, "c", nil)]
puts Views::Widgets.sorted_json(widgets)
"#,
    );
    assert_eq!(
        out,
        r#"{"sorted":[{"id":2,"meta":{"size":null}},{"id":1,"meta":{"size":5}},{"id":3,"meta":{"size":null}}]}"#
    );
}
