//! A `json.<key> do … end` whose block sets nothing is left out.
//!
//! Jbuilder renders the block into a fresh scope; when nothing in it
//! sets a pair (each one is under a condition that is false, or a
//! partial that sets nothing), `_merge_block` answers BLANK and
//! `_set_value` skips the key. The lowering wrote `"<key>":{}` for it
//! every time.
//!
//! The key is now taken back at run time, with its comma, when the
//! block may set nothing and its object comes out `{}`. A block that
//! always sets a pair is emitted as before, with no check.
//!
//! Two layers: the emitted Ruby, and the templates rendered on CRuby
//! (the emitted view modules plus the runtime's `JsonBuilder`, with
//! plain Structs for records) against what Rails 8.1 + jbuilder 2.15.1
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

const TEMPLATES: &[&str] = &["blank", "blank_first", "blank_partial", "blank_nested", "filled"];

/// The block's only pair is under a condition.
const BLANK: &str = r#"json.id @widget.id
json.owner do
  json.name @widget.name unless @widget.name.nil?
end
json.size @widget.size
"#;

/// The same, as the object's first key: the next pair's comma depends
/// on whether the key stayed.
const BLANK_FIRST: &str = r#"json.extra do
  json.note @widget.name if @widget.size
end
json.id @widget.id
"#;

/// The block is a partial that sets nothing for a nil local.
const BLANK_PARTIAL: &str = r#"json.id @widget.id
json.gadget do
  json.partial! "widgets/gadget", gadget: @widget.gadget
end
"#;

const GADGET: &str = r#"json.label gadget.label if gadget
"#;

/// A blank block inside a blank block: both keys go.
const BLANK_NESTED: &str = r#"json.outer do
  json.inner do
    json.name @widget.name if @widget.name
  end
end
"#;

/// A block that always sets a pair, nil or not: always kept.
const FILLED: &str = r#"json.id @widget.id
json.owner do
  json.name @widget.name
end
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
        ("app/views/widgets/blank.json.jbuilder", BLANK),
        ("app/views/widgets/blank_first.json.jbuilder", BLANK_FIRST),
        ("app/views/widgets/blank_partial.json.jbuilder", BLANK_PARTIAL),
        ("app/views/widgets/_gadget.json.jbuilder", GADGET),
        ("app/views/widgets/blank_nested.json.jbuilder", BLANK_NESTED),
        ("app/views/widgets/filled.json.jbuilder", FILLED),
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
fn only_a_block_that_may_set_nothing_is_checked() {
    let files = emitted();
    let src = view(&files, "widgets/blank_json.rb");
    assert!(
        src.contains("io_key0 = io.length") && src.contains("io.slice!(io_key0, io.length) if io.end_with?(\"{}\")"),
        "the key is taken back when its object is empty:\n{src}"
    );
    let src = view(&files, "widgets/filled_json.rb");
    assert!(
        !src.contains("io_key") && !src.contains("end_with?"),
        "a block that always sets a pair has no check:\n{src}"
    );
    assert!(
        src.contains("io << \"\\\"owner\\\":\"\n") && src.contains("io << \"{\""),
        "and is emitted as before:\n{src}"
    );
}

/// Render the templates on CRuby for a widget with every field and one
/// with none, and compare with what Rails 8.1.3 + jbuilder 2.15.1
/// answer for the same rows.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-blank-block-{}",
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
        r#"{"blank":[{"id":1,"owner":{"name":"b"},"size":5},{"id":2,"size":null}],"#,
        r#""blank_first":[{"extra":{"note":"b"},"id":1},{"id":2}],"#,
        r#""blank_partial":[{"id":1,"gadget":{"label":"g"}},{"id":2}],"#,
        r#""blank_nested":[{"outer":{"inner":{"name":"b"}}},{}],"#,
        r#""filled":[{"id":1,"owner":{"name":"b"}},{"id":2,"owner":{"name":null}}]}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
