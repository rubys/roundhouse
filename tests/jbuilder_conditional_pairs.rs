//! Pairs under a condition: `if … else … end`, `unless`, and the
//! `if` / `unless` modifiers.
//!
//! A statement of an object template that is not a `json.*` call was
//! Unknown, so the whole conditional became an empty append: the pairs
//! inside it were dropped with no diagnostic. An optional field
//! (`json.label x if …`) or a branch between two values is the usual
//! way a template writes either.
//!
//! The commas: a conditional with a pair in one branch only leaves the
//! next pair not knowing whether it is the first. Where that matters
//! (`first_pair` below) the comma is decided when the template runs.
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
  get "widgets/:id/branch", to: "widgets#branch", defaults: { format: :json }
  get "widgets/:id/modifier", to: "widgets#modifier", defaults: { format: :json }
  get "widgets/:id/first_pair", to: "widgets#first_pair", defaults: { format: :json }
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def branch
    @widget = Widget.find(params[:id])
    render :branch
  end

  def modifier
    @widget = Widget.find(params[:id])
    render :modifier
  end

  def first_pair
    @widget = Widget.find(params[:id])
    render :first_pair
  end
end
"#;

/// A pair in each branch of a block `if`.
const BRANCH: &str = r#"json.id @widget.id
if @widget.size
  json.big true
else
  json.big false
end
"#;

/// The `if` and `unless` modifiers.
const MODIFIER: &str = r#"json.id @widget.id
json.label @widget.name if @widget.size
json.note @widget.name unless @widget.size.nil?
"#;

/// A pair in one branch only, before any other pair.
const FIRST_PAIR: &str = r#"if @widget.size
  json.big true
end
json.id @widget.id
"#;

fn emitted() -> Vec<(String, String)> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("app/views/widgets/branch.json.jbuilder", BRANCH),
        ("app/views/widgets/modifier.json.jbuilder", MODIFIER),
        ("app/views/widgets/first_pair.json.jbuilder", FIRST_PAIR),
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
fn the_pairs_of_a_conditional_are_emitted_inside_it() {
    let files = emitted();
    let src = view(&files, "widgets/branch_json.rb");
    assert!(
        src.contains("if widget.size") && src.matches("\\\"big\\\":").count() == 2,
        "both branches, each with its pair:\n{src}"
    );
    assert!(!src.contains("io << \"\""), "nothing is dropped:\n{src}");
    let src = view(&files, "widgets/modifier_json.rb");
    assert!(
        src.contains("\\\"label\\\":") && src.contains("\\\"note\\\":"),
        "the modified statements:\n{src}"
    );
    assert!(!src.contains("io << \"\""), "nothing is dropped:\n{src}");
}

/// Render the templates on CRuby for a widget with a size and one
/// without, and compare with what Rails 8.1.4 + jbuilder 2.15.1 answer
/// for the same rows.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-conditional-pairs-{}",
        std::process::id()
    ));
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    let mut requires = String::new();
    for name in ["branch_json.rb", "modifier_json.rb", "first_pair_json.rb"] {
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
  %w[branch modifier first_pair].to_h do |name|
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
        r#"{"branch":[{"id":1,"big":true},{"id":2,"big":false}],"#,
        r#""modifier":[{"id":1,"label":"b","note":"b"},{"id":2}],"#,
        r#""first_pair":[{"big":true,"id":1},{"id":2}]}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
