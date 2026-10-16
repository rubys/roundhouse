//! Pairs inside a block passed to a call that is not on `json`.
//!
//! `ActsAsTenant.without_tenant do … end`, `I18n.with_locale(…) do … end`
//! or any method that yields runs its block with the same builder, so the
//! block's pairs belong to the enclosing object. The statement was
//! Unknown: the call and every pair in its block were dropped, with no
//! diagnostic.
//!
//! The call now stays as written, its block's body lowered against the
//! same accumulator. The block may run any number of times, so its first
//! pair, and the pair after the call, decide their comma at run time
//! unless one is already known. A block the lowerer cannot rebuild (a
//! block parameter named `json`) is reported and raises where it stood.
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

const TEMPLATES: &[&str] = &["around", "around_first", "around_never"];

/// A key block inside the call, after a pair.
const AROUND: &str = r#"json.id @widget.id
Quietly.run do
  json.owner do
    json.name @widget.name
  end
end
"#;

/// The call first, its only pair under a condition.
const AROUND_FIRST: &str = r#"Quietly.run do
  json.name @widget.name if @widget.name
end
json.id @widget.id
"#;

/// A call that never runs its block.
const AROUND_NEVER: &str = r#"Never.run do
  json.never true
end
json.id @widget.id
"#;

/// A block whose parameter shadows the builder: not lowered, reported.
const SHADOWED: &str = r#"json.id @widget.id
Quietly.run do |json|
  json.name @widget.name
end
"#;

fn emitted() -> Vec<(String, String)> {
    let mut routes = String::from("Rails.application.routes.draw do\n");
    let mut actions = String::new();
    for t in TEMPLATES.iter().chain(&["shadowed"]) {
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
        ("app/views/widgets/around.json.jbuilder", AROUND),
        ("app/views/widgets/around_first.json.jbuilder", AROUND_FIRST),
        ("app/views/widgets/around_never.json.jbuilder", AROUND_NEVER),
        ("app/views/widgets/shadowed.json.jbuilder", SHADOWED),
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
fn the_call_stays_around_its_lowered_block() {
    let files = emitted();
    let src = view(&files, "widgets/around_json.rb");
    assert!(
        src.contains("Quietly.run do") && src.contains("\\\"owner\\\":"),
        "the call, with the block's pairs inside it:\n{src}"
    );
    assert!(!src.contains("io << \"\""), "nothing is dropped:\n{src}");
    let src = view(&files, "widgets/shadowed_json.rb");
    assert!(
        src.contains("raise") && src.contains("jbuilder block on a non-json call"),
        "a block that cannot be lowered raises with the reason:\n{src}"
    );
}

/// Render the templates on CRuby for a widget with every field and one
/// with none, and compare with what Rails 8.1.3 + jbuilder 2.15.1
/// answer for the same rows.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-block-call-{}",
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
    for name in TEMPLATES.iter().map(|t| format!("{t}_json.rb")) {
        let file = dir.join("app/views/widgets").join(name);
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r##"require {runtime:?}
{requires}
require "json"
module Quietly; def self.run; yield; end; end
module Never; def self.run; end; end
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
        r#"{"around":[{"id":1,"owner":{"name":"b"}},{"id":2,"owner":{"name":null}}],"#,
        r#""around_first":[{"name":"b","id":1},{"id":2}],"#,
        r#""around_never":[{"id":1},{"id":2}]}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
