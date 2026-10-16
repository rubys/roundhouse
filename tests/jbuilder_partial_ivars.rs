//! An instance variable a jbuilder partial reads (#540).
//!
//! A Rails partial sees the controller's instance variables, as every
//! view of the request does. The lowered partial rewrote `@note` to a
//! bare `note` like any template, but took no parameter for it — only
//! an action template's ivars became parameters — so `check` reported
//! `error[ivar_unresolved]` and, past it, the emitted partial raised
//! NameError.
//!
//! A jbuilder template's ivars are now its closure: its own reads and
//! those of every partial it renders, transitively. An action template
//! takes them all as parameters and the controller passes them; a
//! partial takes the ones it reads after its locals, and each render
//! passes them on. The analyzer follows the same `json.partial!` /
//! `json.array!` edges, so the partial is typed with its renderers'
//! ivars.
//!
//! Three layers: the diagnostics, the emitted Ruby (views and the
//! controller's render call), and the templates rendered on CRuby
//! against what Rails 8.1 + jbuilder 2.15 render for the same
//! templates, row and ivars.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::analyze::{diagnose, DiagnosticKind};
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
  get "gadgets/:id/framed", to: "gadgets#framed"
  get "gadgets/:id/plain", to: "gadgets#plain"
  get "gadgets/:id/carded", to: "gadgets#carded"
end
"#;

const CONTROLLER: &str = r#"class GadgetsController < ApplicationController
  def index
    @widgets = Widget.all
    @note = "hello"
  end

  def show
    @widget = Widget.find_by(id: params[:id])
    @note = "hello"
  end

  def framed
    @widget = Widget.find_by(id: params[:id])
    @note = "hello"
    @title = "Gadget"
  end

  def plain
    @widget = Widget.find_by(id: params[:id])
  end

  def carded
    @widget = Widget.find_by(id: params[:id])
    @note = "hello"
  end
end
"#;

/// The issue's partial: a local, and an ivar of the controller's.
const GADGET: &str = "json.name gadget.name\njson.note @note if @note\n";

/// A partial that reads an ivar and renders the one above, which reads
/// another.
const FRAME: &str = r#"json.title @title
json.inner do
  json.partial! "gadgets/gadget", gadget: gadget
end
"#;

/// A partial reading a local and an ivar of one name, which Rails
/// keeps apart: `note` is what the caller passed, `@note` the
/// controller's. It renders a partial that reads `@note` too.
const CARD: &str = r#"json.local note
json.ivar @note
json.inner do
  json.partial! "gadgets/gadget", gadget: gadget
end
"#;

/// The issue's shape.
const SHOW: &str = "json.partial!(\"gadgets/gadget\", gadget: @widget)\n";

/// A collection: each element's partial reads the ivar.
const INDEX: &str = "json.array! @widgets, partial: \"gadgets/gadget\", as: :gadget\n";

/// Two partials deep.
const FRAMED: &str = "json.partial! \"gadgets/frame\", gadget: @widget\n";

/// The action never sets `@note`: it is nil, in Rails as here.
const PLAIN: &str = "json.partial! \"gadgets/gadget\", gadget: @widget\n";

/// Passes `note` a value of its own while the action sets `@note`.
const CARDED: &str = "json.partial! \"gadgets/card\", gadget: @widget, note: \"local\"\n";

const TEMPLATES: &[(&str, &str)] = &[
    ("show", SHOW),
    ("index", INDEX),
    ("framed", FRAMED),
    ("plain", PLAIN),
    ("carded", CARDED),
];

fn app() -> roundhouse::App {
    app_with(&[])
}

fn app_with(extra: &[(&str, &str)]) -> roundhouse::App {
    let mut files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/gadgets_controller.rb", CONTROLLER),
        ("app/views/gadgets/_gadget.json.jbuilder", GADGET),
        ("app/views/gadgets/_frame.json.jbuilder", FRAME),
        ("app/views/gadgets/_card.json.jbuilder", CARD),
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
    for (path, source) in extra {
        files.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted(files: Vec<roundhouse::emit::EmittedFile>) -> Vec<(String, String)> {
    files
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
                "no emitted file ends with {suffix}; got {:?}",
                files.iter().map(|(p, _)| p).collect::<Vec<_>>()
            )
        })
}

/// The partials are typed with their renderers' ivars: nothing is
/// unresolved.
#[test]
fn a_partial_ivar_resolves_through_its_renderers() {
    let app = app();
    let unresolved: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::IvarUnresolved { name } => Some(name.as_str().to_string()),
            _ => None,
        })
        .collect();
    assert!(unresolved.is_empty(), "ivar_unresolved: {unresolved:?}");
}

/// An ivar no action that renders the partial sets has no type to
/// take: it stays reported, not silently nil.
#[test]
fn an_ivar_no_renderer_sets_stays_reported() {
    let app = app_with(&[
        ("app/views/gadgets/_badge.json.jbuilder", "json.badge @badge\n"),
        ("app/views/gadgets/plain.json.jbuilder", "json.partial! \"gadgets/badge\"\n"),
    ]);
    let unresolved: Vec<String> = diagnose(&app)
        .into_iter()
        .filter_map(|d| match d.kind {
            DiagnosticKind::IvarUnresolved { name } => Some(name.as_str().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(unresolved, ["badge"]);
}

/// Each partial takes the ivars it reads after its locals; each render
/// passes them on; each action template takes its whole closure, and
/// the controller passes it.
#[test]
fn the_ivars_are_threaded_from_the_controller_to_the_partial() {
    let app = app();
    let views = emitted(ruby::emit_lowered_jbuilder_views(&app));
    for (file, text) in [
        ("gadgets/_gadget_json.rb", "def self.gadget_json(gadget, note)"),
        ("gadgets/_frame_json.rb", "def self.frame_json(gadget, title, note)"),
        ("gadgets/_frame_json.rb", "Views::Gadgets.gadget_json(gadget, note)"),
        ("gadgets/show_json.rb", "def self.show_json(widget, note)"),
        ("gadgets/show_json.rb", "Views::Gadgets.gadget_json(widget, note)"),
        ("gadgets/index_json.rb", "def self.index_json(widgets, note)"),
        ("gadgets/index_json.rb", "Views::Gadgets.gadget_json(gadget, note)"),
        ("gadgets/framed_json.rb", "def self.framed_json(widget, title, note)"),
        ("gadgets/framed_json.rb", "Views::Gadgets.frame_json(widget, title, note)"),
        ("gadgets/plain_json.rb", "def self.plain_json(widget, note)"),
    ] {
        let src = view(&views, file);
        assert!(src.contains(text), "{file}: expected `{text}`:\n{src}");
    }
    let controllers = emitted(ruby::emit_lowered_controllers(&app));
    let controller = view(&controllers, "gadgets_controller.rb");
    for call in [
        "Views::Gadgets.show_json(@widget, @note)",
        "Views::Gadgets.index_json(@widgets, @note)",
        "Views::Gadgets.framed_json(@widget, @title, @note)",
        "Views::Gadgets.plain_json(@widget, @note)",
    ] {
        assert!(controller.contains(call), "expected `{call}`:\n{controller}");
    }
}

/// A local and an ivar of one name are two parameters: the ivar's
/// takes the generated `__rh_ivar_` name, `@note` reads it and bare
/// `note` the local, and the partial passes its own renders the ivar.
/// A name that collides with nothing keeps the ivar's own name.
#[test]
fn a_local_and_an_ivar_of_one_name_stay_apart() {
    let app = app();
    let views = emitted(ruby::emit_lowered_jbuilder_views(&app));
    for (file, text) in [
        ("gadgets/_card_json.rb", "def self.card_json(gadget, note, __rh_ivar_note)"),
        ("gadgets/_card_json.rb", "Views::Gadgets.gadget_json(gadget, __rh_ivar_note)"),
        ("gadgets/carded_json.rb", "def self.carded_json(widget, note)"),
        ("gadgets/carded_json.rb", "Views::Gadgets.card_json(widget, \"local\", note)"),
        ("gadgets/_gadget_json.rb", "def self.gadget_json(gadget, note)"),
    ] {
        let src = view(&views, file);
        assert!(src.contains(text), "{file}: expected `{text}`:\n{src}");
    }
    let card = view(&views, "gadgets/_card_json.rb");
    for text in ["encode_value(note)", "encode_value(__rh_ivar_note)"] {
        assert!(card.contains(text), "expected `{text}`:\n{card}");
    }
    let controllers = emitted(ruby::emit_lowered_controllers(&app));
    let controller = view(&controllers, "gadgets_controller.rb");
    let call = "Views::Gadgets.carded_json(@widget, @note)";
    assert!(controller.contains(call), "expected `{call}`:\n{controller}");
}

/// Render the templates on CRuby with the ivars each action sets and
/// compare with what Rails 8.1.4 + jbuilder 2.15.1 answer.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let app = app();
    let files = emitted(ruby::emit_lowered_jbuilder_views(&app));
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-partial-ivars-{}",
        std::process::id()
    ));
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let mut requires = String::new();
    let names = ["_gadget", "_frame", "_card"].into_iter().chain(TEMPLATES.iter().map(|(n, _)| *n));
    for name in names {
        let file = dir.join("app/views/gadgets").join(format!("{name}_json.rb"));
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r#"require {runtime:?}
{requires}
require "json"
Widget = Struct.new(:id, :name)
widget = Widget.new(1, "Sprocket")
puts JSON.generate(
  "show" => JSON.parse(Views::Gadgets.show_json(widget, "hello")),
  "index" => JSON.parse(Views::Gadgets.index_json([widget], "hello")),
  "framed" => JSON.parse(Views::Gadgets.framed_json(widget, "Gadget", "hello")),
  "plain" => JSON.parse(Views::Gadgets.plain_json(widget, nil)),
  "carded" => JSON.parse(Views::Gadgets.carded_json(widget, "hello")),
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
    let expected = concat!(
        r#"{"show":{"name":"Sprocket","note":"hello"},"#,
        r#""index":[{"name":"Sprocket","note":"hello"}],"#,
        r#""framed":{"title":"Gadget","inner":{"name":"Sprocket","note":"hello"}},"#,
        r#""plain":{"name":"Sprocket"},"#,
        r#""carded":{"local":"local","ivar":"hello","inner":{"name":"Sprocket","note":"hello"}}}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
