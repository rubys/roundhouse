//! A jbuilder partial's locals are the names its callers pass (#539).
//!
//! Rails binds a partial's locals by the keys the call passes, or by
//! the `as:` name of a collection or one-record render, never by the
//! partial's own file name. The lowered partial took one parameter
//! named after its file (`_gadget` → `gadget`), and each call passed
//! one value positionally, so a partial that reads `widget` because its
//! caller passed `widget:` read a local nothing defined: a NameError on
//! the first request.
//!
//! The partial now takes one parameter per local its callers pass, and
//! each call passes each parameter its value under that name, `nil`
//! for one only another caller passes (which the partial reads through
//! `local_assigns[:x]`, as nil in Rails too).
//!
//! Two layers: the emitted Ruby, and the templates rendered on CRuby
//! (the emitted view modules plus the runtime's `JsonBuilder`, with a
//! plain Struct for the record) against what Rails 8.1 + jbuilder 2.15
//! render for the same templates and row.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const SCHEMA: &str = r#"ActiveRecord::Schema.define do
  create_table "widgets", force: :cascade do |t|
    t.string "name"
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  resources :gadgets, only: %i[index show]
end
"#;

const CONTROLLER: &str = r#"class GadgetsController < ApplicationController
  def index
    @widgets = Widget.all
  end

  def show
    @widget = Widget.find(params[:id])
  end
end
"#;

/// Reads the local its callers pass as `widget:`, and an optional
/// `label` only one of them passes.
const GADGET: &str = r#"json.name widget.name
json.label local_assigns[:label] if local_assigns[:label]
"#;

/// Reads whichever of two locals the caller passed.
const TILE: &str = "json.name (local_assigns[:widget] || local_assigns[:gadget]).name\n";

/// The issue's shape: a locals key that is not the partial's name.
const SHOW: &str = "json.partial!(\"gadgets/gadget\", widget: @widget)\n";

/// Both locals, in the other order.
const FEATURED: &str = "json.partial! \"gadgets/gadget\", label: \"featured\", widget: @widget\n";

/// An explicit `locals:` Hash.
const EXPLICIT: &str = "json.partial! \"gadgets/gadget\", locals: { widget: @widget, label: \"x\" }\n";

/// The options Hash alone.
const OPTIONS: &str = "json.partial! partial: \"gadgets/gadget\", widget: @widget\n";

/// A collection, `as:` the local's name.
const INDEX: &str = "json.array! @widgets, partial: \"gadgets/gadget\", as: :widget\n";

/// The same collection, with a positional path.
const LISTING: &str = "json.partial! \"gadgets/gadget\", collection: @widgets, as: :widget\n";

/// One record under a key.
const WRAPPED: &str = "json.kind \"wrapped\"\njson.item @widget, partial: \"gadgets/gadget\", as: :widget\n";

/// A collection block whose element is the partial.
const BLOCK: &str = "json.items @widgets do |w|\n  json.partial! \"gadgets/gadget\", widget: w\nend\n";

/// Two callers of one partial with different keys.
const TILE_ONE: &str = "json.partial! \"gadgets/tile\", widget: @widget\n";
const TILE_MANY: &str = "json.array! @widgets, partial: \"gadgets/tile\", as: :gadget\n";

const TEMPLATES: &[(&str, &str)] = &[
    ("show", SHOW),
    ("featured", FEATURED),
    ("explicit", EXPLICIT),
    ("options", OPTIONS),
    ("index", INDEX),
    ("listing", LISTING),
    ("wrapped", WRAPPED),
    ("block", BLOCK),
    ("tile_one", TILE_ONE),
    ("tile_many", TILE_MANY),
];

fn emitted() -> Vec<(String, String)> {
    let mut files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/gadgets_controller.rb", CONTROLLER),
        ("app/views/gadgets/_gadget.json.jbuilder", GADGET),
        ("app/views/gadgets/_tile.json.jbuilder", TILE),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    for (name, source) in TEMPLATES {
        files.insert(
            PathBuf::from(format!("app/views/gadgets/{name}.json.jbuilder")),
            source.as_bytes().to_vec(),
        );
    }
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

/// The partial takes the locals its callers pass, by those names, and
/// each call passes each one its own value or nil.
#[test]
fn a_partial_takes_the_locals_its_callers_pass() {
    let files = emitted();
    let gadget = view(&files, "gadgets/_gadget_json.rb");
    assert!(gadget.contains("def self.gadget_json(widget, label)"), "{gadget}");
    assert!(gadget.contains("widget.name"), "{gadget}");
    assert!(!gadget.contains("local_assigns"), "{gadget}");

    let tile = view(&files, "gadgets/_tile_json.rb");
    assert!(tile.contains("def self.tile_json(gadget, widget)"), "{tile}");

    for (template, call) in [
        ("show", "Views::Gadgets.gadget_json(widget, nil)"),
        ("featured", "Views::Gadgets.gadget_json(widget, \"featured\")"),
        ("explicit", "Views::Gadgets.gadget_json(widget, \"x\")"),
        ("options", "Views::Gadgets.gadget_json(widget, nil)"),
        ("index", "Views::Gadgets.gadget_json(widget, nil)"),
        ("listing", "Views::Gadgets.gadget_json(widget, nil)"),
        ("wrapped", "Views::Gadgets.gadget_json(widget, nil)"),
        ("block", "Views::Gadgets.gadget_json(w, nil)"),
        ("tile_one", "Views::Gadgets.tile_json(nil, widget)"),
        ("tile_many", "Views::Gadgets.tile_json(gadget, nil)"),
    ] {
        let src = view(&files, &format!("gadgets/{template}_json.rb"));
        assert!(src.contains(call), "{template}: expected `{call}`:\n{src}");
    }
}

/// Render the templates on CRuby and compare with what Rails 8.1.4 +
/// jbuilder 2.15.1 answer for the same row.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-partial-locals-{}",
        std::process::id()
    ));
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let mut requires = String::new();
    let names = ["_gadget", "_tile"].into_iter().chain(TEMPLATES.iter().map(|(n, _)| *n));
    for name in names {
        let file = dir.join("app/views/gadgets").join(format!("{name}_json.rb"));
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let mut renders = String::new();
    for (name, source) in TEMPLATES {
        let arg = if source.contains("@widgets") { "widgets" } else { "widget" };
        renders.push_str(&format!(
            "  {name:?} => JSON.parse(Views::Gadgets.{name}_json({arg})),\n"
        ));
    }
    let script = format!(
        r#"require {runtime:?}
{requires}
require "json"
Widget = Struct.new(:id, :name)
widget = Widget.new(1, "Sprocket")
widgets = [widget]
puts JSON.generate(
{renders})
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
        r#"{"show":{"name":"Sprocket"},"#,
        r#""featured":{"name":"Sprocket","label":"featured"},"#,
        r#""explicit":{"name":"Sprocket","label":"x"},"#,
        r#""options":{"name":"Sprocket"},"#,
        r#""index":[{"name":"Sprocket"}],"#,
        r#""listing":[{"name":"Sprocket"}],"#,
        r#""wrapped":{"kind":"wrapped","item":{"name":"Sprocket"}},"#,
        r#""block":{"items":[{"name":"Sprocket"}]},"#,
        r#""tile_one":{"name":"Sprocket"},"#,
        r#""tile_many":[{"name":"Sprocket"}]}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
