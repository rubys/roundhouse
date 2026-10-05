//! `json.partial! @record` (#322), and the record with options,
//! `json.partial!(@record, partial: "…", as: :x)`.
//!
//! Jbuilder's `partial!` with one Active Model argument renders the
//! record's own partial, `record.to_partial_path` (`widgets/_widget`
//! for a `Widget`), with the record as the partial's local. With
//! options after the record it renders the same partial: `partial!`
//! sets the `partial:` option to its positional. The `partial!` arm
//! wanted a string-literal path first, so both calls were Unknown and
//! the template rendered `{}`.
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
    t.integer "size"
  end
end
"#;

const ROUTES: &str = r#"Rails.application.routes.draw do
  get "widgets/:id/detail", to: "widgets#detail", defaults: { format: :json }
  get "widgets/:id/wrapped", to: "widgets#wrapped", defaults: { format: :json }
  get "widgets/:id/aliased", to: "widgets#aliased", defaults: { format: :json }
  get "widgets/:id/stray", to: "widgets#stray", defaults: { format: :json }
  get "widgets/:id/extra", to: "widgets#extra", defaults: { format: :json }
  get "widgets/:id/entry", to: "widgets#entry", defaults: { format: :json }
  namespace :admin do
    get "widgets/:id", to: "widgets#show", defaults: { format: :json }
  end
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def detail
    @widget = Widget.find(params[:id])
    render :detail
  end

  def wrapped
    @widget = Widget.find(params[:id])
    render :wrapped
  end

  def aliased
    @widget = Widget.find(params[:id])
    render :aliased
  end

  def stray
    @gadget = Widget.find(params[:id])
    render :stray
  end

  def extra
    @widget = Widget.find(params[:id])
    render :extra
  end

  def entry
    @widget = Widget.find(params[:id])
    render :entry
  end
end
"#;

const PARTIAL: &str = "json.id widget.id\njson.name widget.name\n";

/// The whole template.
const DETAIL: &str = "json.partial! @widget\n";

/// The value of a nested object.
const WRAPPED: &str = r#"json.kind "wrapped"
json.widget do
  json.partial! @widget
end
"#;

/// The record first, with options.
const ALIASED: &str = "json.partial!(@widget, partial: \"widgets/widget\", as: :widget)\n";

/// A local that names no model of the app.
const STRAY: &str = "json.partial! @gadget\n";

/// A local for the partial besides the record.
const EXTRA: &str = "json.partial! @widget, as: :widget, label: \"x\"\n";

/// `as:` names the partial's local something other than `widget`.
const ENTRY: &str = "json.partial! @widget, as: :entry\n";

/// `as:` spelled as a String: Action View `to_sym`s it, so these are
/// `as: :entry` and `as: :widget`. A non-literal `as:` is a name known
/// only at run time.
const ENTRY_STRING: &str = "json.partial! @widget, as: \"entry\"\n";
const WIDGET_STRING: &str = "json.partial! @widget, as: \"widget\"\n";
const DYNAMIC_AS: &str = "json.partial! @widget, as: local_name\n";

/// A namespaced controller's view: Action View prefixes the record's
/// partial path with the controller's namespace.
const ADMIN_CONTROLLER: &str = r#"class Admin::WidgetsController < ApplicationController
  def show
    @widget = Widget.find(params[:id])
    render :show
  end
end
"#;

const ADMIN_SHOW: &str = "json.partial! @widget\n";

const ADMIN_PARTIAL: &str = "json.id widget.id\njson.admin true\n";

fn emitted() -> Vec<(String, String)> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", ROUTES),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("app/views/widgets/_widget.json.jbuilder", PARTIAL),
        ("app/views/widgets/detail.json.jbuilder", DETAIL),
        ("app/views/widgets/wrapped.json.jbuilder", WRAPPED),
        ("app/views/widgets/aliased.json.jbuilder", ALIASED),
        ("app/views/widgets/stray.json.jbuilder", STRAY),
        ("app/views/widgets/extra.json.jbuilder", EXTRA),
        ("app/views/widgets/entry.json.jbuilder", ENTRY),
        ("app/views/widgets/entry_string.json.jbuilder", ENTRY_STRING),
        ("app/views/widgets/widget_string.json.jbuilder", WIDGET_STRING),
        ("app/views/widgets/dynamic_as.json.jbuilder", DYNAMIC_AS),
        ("app/controllers/admin/widgets_controller.rb", ADMIN_CONTROLLER),
        ("app/views/admin/widgets/show.json.jbuilder", ADMIN_SHOW),
        ("app/views/admin/widgets/_widget.json.jbuilder", ADMIN_PARTIAL),
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
fn a_record_renders_its_models_partial() {
    let files = emitted();
    let src = view(&files, "widgets/detail_json.rb");
    assert!(
        src.contains("io << Views::Widgets.widget_json(widget)"),
        "the record's partial, with the record:\n{src}"
    );
    let src = view(&files, "widgets/wrapped_json.rb");
    assert!(
        src.contains("io << Views::Widgets.widget_json(widget)"),
        "the same call as a nested object's value:\n{src}"
    );
    let src = view(&files, "admin/widgets/show_json.rb");
    assert!(
        src.contains("io << Views::Admin::Widgets.widget_json(widget)"),
        "a namespaced view renders the namespaced partial:\n{src}"
    );
    let src = view(&files, "widgets/aliased_json.rb");
    assert!(
        src.contains("io << Views::Widgets.widget_json(widget)"),
        "the same call with options after the record:\n{src}"
    );
}

/// The path comes from the model the local is named after. A local
/// that names no model of the app has no path to take; a local for the
/// partial (`label:`) has no way through a positional call; and an
/// `as:` other than `widget` names a local the lowered partial does not
/// take its record under. All three stay unrecognized, as before,
/// rather than calling a partial that may not exist or that would miss
/// its local.
#[test]
fn a_record_with_no_model_or_with_extra_locals_is_left_alone() {
    let files = emitted();
    for template in ["stray", "extra", "entry"] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            !src.contains("Views::Widgets.") && src.contains("io << \"\""),
            "{template}: no partial call is emitted:\n{src}"
        );
    }
}

/// A String `as:` is compared like the Symbol: `as: "entry"` is left
/// alone as `as: :entry` is, rather than binding the record as `widget`
/// for a partial that reads `entry`; `as: "widget"` renders the partial.
/// An `as:` that is not a literal is left alone too.
#[test]
fn a_string_as_is_compared_like_a_symbol() {
    let files = emitted();
    for template in ["entry_string", "dynamic_as"] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            !src.contains("Views::Widgets.") && src.contains("io << \"\""),
            "{template}: no partial call is emitted:\n{src}"
        );
    }
    let src = view(&files, "widgets/widget_string_json.rb");
    assert!(
        src.contains("io << Views::Widgets.widget_json(widget)"),
        "as: \"widget\" renders the record's partial:\n{src}"
    );
}

/// Render the templates on CRuby and compare with what Rails 8.1.4 +
/// jbuilder 2.15.1 answer for the same row.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!(
        "roundhouse-jbuilder-partial-record-{}",
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
    for name in ["_widget_json.rb", "detail_json.rb", "wrapped_json.rb", "aliased_json.rb"] {
        let file = dir.join("app/views/widgets").join(name);
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r#"require {runtime:?}
{requires}
require "json"
Widget = Struct.new(:id, :name, :size)
widget = Widget.new(1, "b", 5)
puts JSON.generate(
  "detail" => JSON.parse(Views::Widgets.detail_json(widget)),
  "wrapped" => JSON.parse(Views::Widgets.wrapped_json(widget)),
  "aliased" => JSON.parse(Views::Widgets.aliased_json(widget)),
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
        r#"{"detail":{"id":1,"name":"b"},"#,
        r#""wrapped":{"kind":"wrapped","widget":{"id":1,"name":"b"}},"#,
        r#""aliased":{"id":1,"name":"b"}}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
