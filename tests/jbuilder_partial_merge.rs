//! `json.partial!` next to pairs, or under an `if` / `unless`.
//!
//! Jbuilder renders the partial into the current object, so its pairs
//! join the object's own: `json.gadget do json.partial! "widgets/gadget",
//! gadget: g if g end` is `"gadget":{"label":…}`. The object walker wrote
//! such a partial as an empty append, and a collection element mixing a
//! partial with other statements was refused.
//!
//! The partial is now rendered and its object, without the braces, is
//! appended with a run-time comma; a partial that sets nothing adds
//! nothing.
//!
//! Two layers: the emitted Ruby, and the templates rendered on CRuby
//! against what Rails 8.1.3 + jbuilder 2.15.1 render for the same
//! templates and rows.

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

const TEMPLATES: &[&str] =
    &["partial_if", "partial_unless", "partial_mixed", "partial_in_object", "mixed_element"];

/// The partial under an `if` modifier, alone in a key block.
const PARTIAL_IF: &str = r#"json.id @widget.id
json.gadget do
  json.partial! "widgets/gadget", gadget: @widget.gadget if @widget.gadget
end
"#;

/// Under `unless`, as the object's first key.
const PARTIAL_UNLESS: &str = r#"json.gadget do
  json.partial! "widgets/gadget", gadget: @widget.gadget unless @widget.gadget.nil?
end
json.id @widget.id
"#;

/// Between two pairs of a key block.
const PARTIAL_MIXED: &str = r#"json.id @widget.id
json.gadget do
  json.kind "g"
  json.partial! "widgets/gadget", gadget: @widget.gadget
  json.size @widget.size if @widget.size
end
"#;

/// Under a condition at the template's top level.
const PARTIAL_IN_OBJECT: &str = r#"json.partial! "widgets/gadget", gadget: @widget.gadget if @widget.gadget
json.id @widget.id
"#;

/// Next to a pair in a collection element.
const MIXED_ELEMENT: &str = r#"json.parts [@widget] do |w|
  json.id w.id
  json.partial! "widgets/gadget", gadget: w.gadget
end
"#;

const GADGET: &str = r#"json.label gadget.label if gadget
"#;

fn emitted() -> Vec<(String, String)> {
    let mut routes = String::from("Rails.application.routes.draw do\n");
    let mut actions = String::new();
    for t in TEMPLATES {
        routes.push_str(&format!(
            "  get \"widgets/:id/{t}\", to: \"widgets#{t}\", defaults: {{ format: :json }}\n"
        ));
        actions.push_str(&format!(
            "  def {t}\n    @widget = Widget.find(params[:id])\n    render :{t}\n  end\n\n"
        ));
    }
    routes.push_str("end\n");
    let controller = format!("class WidgetsController < ApplicationController\n{actions}end\n");
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", routes.as_str()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", controller.as_str()),
        ("app/views/widgets/partial_if.json.jbuilder", PARTIAL_IF),
        ("app/views/widgets/partial_unless.json.jbuilder", PARTIAL_UNLESS),
        ("app/views/widgets/partial_mixed.json.jbuilder", PARTIAL_MIXED),
        ("app/views/widgets/partial_in_object.json.jbuilder", PARTIAL_IN_OBJECT),
        ("app/views/widgets/mixed_element.json.jbuilder", MIXED_ELEMENT),
        ("app/views/widgets/_gadget.json.jbuilder", GADGET),
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
fn the_partial_is_rendered_where_it_stands() {
    let files = emitted();
    for t in TEMPLATES {
        let src = view(&files, &format!("widgets/{t}_json.rb"));
        assert!(
            src.contains("Views::Widgets.gadget_json(") && !src.contains("io << \"\""),
            "the partial is called, nothing is dropped:\n{src}"
        );
    }
}

/// Render the templates on CRuby for a widget with every field and one
/// with none, and compare with what Rails 8.1.3 + jbuilder 2.15.1
/// answer for the same rows.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-partial-merge-{}",
        std::process::id()
    ));
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    // The views that call a partial `require_relative` the views index.
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let mut requires = String::new();
    for name in TEMPLATES.iter().map(|t| format!("{t}_json.rb")).chain(["_gadget_json.rb".to_string()]) {
        let file = dir.join("app/views/widgets").join(name);
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r##"require {runtime:?}
{requires}
require "json"
Gadget = Struct.new(:label)
Widget = Struct.new(:id, :name, :size, :gadget)
full = Widget.new(1, "b", 5, Gadget.new("g"))
bare = Widget.new(2, nil, nil, nil)
puts JSON.generate(
  %w[{names}].to_h do |name|
    [name, [full, bare].map {{ |w| JSON.parse(Views::Widgets.public_send("#{{name}}_json", w)) }}]
  end
)
"##,
        runtime = runtime.display().to_string(),
        names = TEMPLATES.join(" "),
    );
    let output = Command::new("ruby").args(["-e", &script]).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Each value is what Rails answers for that template, for the full
    // widget, then the bare one.
    let expected = concat!(
        r#"{"partial_if":[{"id":1,"gadget":{"label":"g"}},{"id":2}],"#,
        r#""partial_unless":[{"gadget":{"label":"g"},"id":1},{"id":2}],"#,
        r#""partial_mixed":[{"id":1,"gadget":{"kind":"g","label":"g","size":5}},{"id":2,"gadget":{"kind":"g"}}],"#,
        r#""partial_in_object":[{"label":"g","id":1},{"id":2}],"#,
        r#""mixed_element":[{"parts":[{"id":1,"label":"g"}]},{"parts":[{"id":2}]}]}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
