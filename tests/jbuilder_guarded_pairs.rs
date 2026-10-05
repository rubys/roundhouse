//! Pairs inside `begin … rescue … end` (#322).
//!
//! A jbuilder statement that is not a `json.*` call was Unknown, so a
//! template or partial whose body is a `begin … rescue … end` rendered
//! `{}`. Jbuilder runs the body, keeps the pairs it finished when it
//! raises, and adds the rescue's pairs after them; the lowering now
//! does the same.
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
  get "widgets/:id/guarded", to: "widgets#guarded", defaults: { format: :json }
  get "widgets/:id/fragile", to: "widgets#fragile", defaults: { format: :json }
  get "widgets/:id/ensured", to: "widgets#ensured", defaults: { format: :json }
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def guarded
    @widget = Widget.find(params[:id])
    render :guarded
  end

  def fragile
    @widget = Widget.find(params[:id])
    render :fragile
  end

  def ensured
    @widget = Widget.find(params[:id])
    render :ensured
  end
end
"#;

/// A partial whose whole body is guarded, rendered by a template.
const GUARDED_PARTIAL: &str = r#"begin
  json.id widget.id
  json.name widget.name
rescue StandardError
  json.error "unavailable"
end
"#;

const GUARDED: &str = "json.partial! \"widgets/guarded_widget\", widget: @widget\n";

/// The second pair raises for a widget with no size; a pair follows
/// the `begin`.
const FRAGILE: &str = r#"begin
  json.id @widget.id
  json.next_size @widget.size.succ
rescue NoMethodError
  json.error "unavailable"
end
json.kind "fragile"
"#;

/// A `begin` with an `ensure`: not lowered (see below).
const ENSURED: &str = r#"begin
  json.id @widget.id
rescue StandardError
  json.error "unavailable"
ensure
  json.done true
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
        ("app/views/widgets/_guarded_widget.json.jbuilder", GUARDED_PARTIAL),
        ("app/views/widgets/guarded.json.jbuilder", GUARDED),
        ("app/views/widgets/fragile.json.jbuilder", FRAGILE),
        ("app/views/widgets/ensured.json.jbuilder", ENSURED),
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
fn the_pairs_of_a_guarded_body_and_its_rescue_are_emitted() {
    let files = emitted();
    let src = view(&files, "widgets/_guarded_widget_json.rb");
    for needle in ["begin", "rescue StandardError", "\\\"id\\\":", "\\\"name\\\":", "\\\"error\\\":"] {
        assert!(src.contains(needle), "{needle} is emitted:\n{src}");
    }
    assert!(!src.contains("io << \"\""), "nothing is dropped:\n{src}");
}

/// Only a `begin` with `rescue` clauses and no `else` / `ensure` is
/// lowered. Any other shape stays on the old path, an unrecognized
/// statement (`io << ""`), and the view still parses (checked above).
#[test]
fn a_begin_with_an_ensure_is_left_alone() {
    let files = emitted();
    let src = view(&files, "widgets/ensured_json.rb");
    assert!(
        !src.contains("begin") && src.contains("io << \"\""),
        "the statement is not lowered:\n{src}"
    );
}

/// Render both templates on CRuby for a widget with a size and one
/// without, and compare with what Rails 8.1.4 + jbuilder 2.15.1 answer
/// for the same rows. `fragile` raises in its second pair for the
/// widget with no size: the first pair stays, the half-written second
/// one does not, and the rescue's pair and the pair after the `begin`
/// follow.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-guarded-pairs-{}",
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
    for name in ["_guarded_widget_json.rb", "guarded_json.rb", "fragile_json.rb"] {
        let file = dir.join("app/views/widgets").join(name);
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r##"require {runtime:?}
{requires}
require "json"
Widget = Struct.new(:id, :name, :size)
sized = Widget.new(1, "b", 5)
unsized = Widget.new(2, "a", nil)
puts JSON.generate(
  %w[guarded fragile].to_h do |name|
    [name, [sized, unsized].map {{ |w| JSON.parse(Views::Widgets.public_send("#{{name}}_json", w)) }}]
  end
)
"##,
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
    // Each value is what the Rails app answers for that template, for
    // the widget with a size, then the one without.
    let expected = concat!(
        r#"{"guarded":[{"id":1,"name":"b"},{"id":2,"name":"a"}],"#,
        r#""fragile":[{"id":1,"next_size":6,"kind":"fragile"},{"id":2,"error":"unavailable","kind":"fragile"}]}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
