//! `json.partial! @record` (#322), and the record with options,
//! `json.partial!(@record, partial: "…", as: :x)` or
//! `json.partial!(@record, cached: true)`, and a record with a literal
//! local its partial never reads (`cache: true`, `label: "x"`).
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
  create_table "gizmos", force: :cascade do |t|
    t.string "name"
  end
  create_table "sprockets", force: :cascade do |t|
    t.string "name"
  end
  create_table "cogs", force: :cascade do |t|
    t.string "name"
  end
  create_table "pinions", force: :cascade do |t|
    t.string "name"
  end
  create_table "ratchets", force: :cascade do |t|
    t.string "name"
  end
  create_table "levers", force: :cascade do |t|
    t.string "name"
  end
  create_table "spindles", force: :cascade do |t|
    t.string "name"
  end
  create_table "cams", force: :cascade do |t|
    t.string "name"
  end
  create_table "pawls", force: :cascade do |t|
    t.string "name"
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
  get "widgets/:id/cached", to: "widgets#cached", defaults: { format: :json }
  get "widgets/:id/aliased_cached", to: "widgets#aliased_cached", defaults: { format: :json }
  get "widgets/:id/unknown", to: "widgets#unknown", defaults: { format: :json }
  get "widgets/:id/cache_option", to: "widgets#cache_option", defaults: { format: :json }
  get "widgets/:id/other_path", to: "widgets#other_path", defaults: { format: :json }
  get "widgets/items_cache", to: "widgets#items_cache", defaults: { format: :json }
  get "widgets/items_cached", to: "widgets#items_cached", defaults: { format: :json }
  get "widgets/:id/gizmo_cache", to: "widgets#gizmo_cache", defaults: { format: :json }
  get "widgets/:id/sprocket_cache", to: "widgets#sprocket_cache", defaults: { format: :json }
  get "widgets/:id/cog_cache", to: "widgets#cog_cache", defaults: { format: :json }
  get "widgets/:id/layout_option", to: "widgets#layout_option", defaults: { format: :json }
  get "widgets/:id/dynamic_cache", to: "widgets#dynamic_cache", defaults: { format: :json }
  get "widgets/:id/dynamic_partial", to: "widgets#dynamic_partial", defaults: { format: :json }
  get "widgets/:id/pinion_cache", to: "widgets#pinion_cache", defaults: { format: :json }
  get "widgets/:id/ratchet_cache", to: "widgets#ratchet_cache", defaults: { format: :json }
  get "widgets/:id/lever_cache", to: "widgets#lever_cache", defaults: { format: :json }
  get "widgets/:id/spindle_cache", to: "widgets#spindle_cache", defaults: { format: :json }
  get "widgets/:id/cam_cache", to: "widgets#cam_cache", defaults: { format: :json }
  get "widgets/:id/pawl_cache", to: "widgets#pawl_cache", defaults: { format: :json }
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

  def cached
    @widget = Widget.find(params[:id])
    render :cached
  end

  def aliased_cached
    @widget = Widget.find(params[:id])
    render :aliased_cached
  end

  def unknown
    @thing = params[:id]
    render :unknown
  end

  def cache_option
    @widget = Widget.find(params[:id])
    render :cache_option
  end

  def other_path
    @widget = Widget.find(params[:id])
    render :other_path
  end

  def items_cache
    @widgets = Widget.all
    render :items_cache
  end

  def items_cached
    @widgets = Widget.all
    render :items_cached
  end

  def gizmo_cache
    @gizmo = Gizmo.find(params[:id])
    render :gizmo_cache
  end

  def sprocket_cache
    @sprocket = Sprocket.find(params[:id])
    render :sprocket_cache
  end

  def cog_cache
    @cog = Cog.find(params[:id])
    render :cog_cache
  end

  def layout_option
    @widget = Widget.find(params[:id])
    render :layout_option
  end

  def dynamic_cache
    @widget = Widget.find(params[:id])
    @flag = params[:flag]
    render :dynamic_cache
  end

  def dynamic_partial
    @widget = Widget.find(params[:id])
    render :dynamic_partial
  end

  def pinion_cache
    @pinion = Pinion.find(params[:id])
    render :pinion_cache
  end

  def ratchet_cache
    @ratchet = Ratchet.find(params[:id])
    render :ratchet_cache
  end

  def lever_cache
    @lever = Lever.find(params[:id])
    render :lever_cache
  end

  def spindle_cache
    @spindle = Spindle.find(params[:id])
    render :spindle_cache
  end

  def cam_cache
    @cam = Cam.find(params[:id])
    render :cam_cache
  end

  def pawl_cache
    @pawl = Pawl.find(params[:id])
    render :pawl_cache
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

/// `cached: true` after the record. Jbuilder takes `cached` out of the
/// partial's locals with its other options; for a single record it
/// only turns on fragment caching, which changes nothing rendered.
const CACHED: &str = "json.partial!(@widget, cached: true)\n";
const ALIASED_CACHED: &str =
    "json.partial!(@widget, partial: \"widgets/widget\", as: :widget, cached: true)\n";

/// A local that names no model of the app, holding a `Widget`.
const STRAY: &str = "json.partial! @gadget\n";

/// A value of no model: its partial cannot be named.
const UNKNOWN: &str = "json.partial!(@thing)\n";

/// A local for the partial besides the record. `widgets/_widget`
/// never reads `label`, so it renders the same partial with the same
/// output as without it.
const EXTRA: &str = "json.partial! @widget, as: :widget, label: \"x\"\n";

/// `cache: true` is not a Jbuilder option: `partial!` passes every key
/// but `partial`, `as`, `collection` and `cached` to the partial as a
/// local, so this is a local `cache` the partial never reads.
const CACHE_OPTION: &str =
    "json.partial!(@widget, partial: \"widgets/widget\", as: :widget, cache: true)\n";

/// A `partial:` other than the record's own: `partial!` overwrites it
/// with the record, and Action View renders `to_partial_path`.
const OTHER_PATH: &str =
    "json.partial!(@widget, partial: \"widgets/card\", as: :widget, cache: true)\n";

/// The same calls once per element of a key's collection.
const ITEMS_CACHE: &str = "json.items(@widgets) { |w| json.partial!(w, partial: \"widgets/widget\", as: :widget, cache: true) }\n";
const ITEMS_CACHED: &str = "json.items(@widgets) { |w| json.partial!(w, partial: \"widgets/widget\", as: :widget, cached: true) }\n";

/// A partial that reads the local the call passes: its output depends
/// on it (`{"id":3,"fresh":true}` from Rails), and a call that dropped
/// it would read Action View's `cache` helper instead.
const GIZMO_CACHE: &str =
    "json.partial!(@gizmo, partial: \"gizmos/gizmo\", as: :gizmo, cache: true)\n";
const GIZMO_PARTIAL: &str = "json.id gizmo.id\njson.fresh cache\n";

/// Partials that read the local the call passes through `binding`
/// without naming it, receiverless and with an explicit `self`.
const SPROCKET_CACHE: &str = "json.partial!(@sprocket, as: :sprocket, cache: true)\n";
const SPROCKET_PARTIAL: &str =
    "json.id sprocket.id\njson.fresh binding.local_variable_get(:cache)\n";
const COG_CACHE: &str = "json.partial!(@cog, as: :cog, cache: true)\n";
const COG_PARTIAL: &str = "json.id cog.id\njson.fresh self.binding.local_variable_get(:cache)\n";

/// The same read reached through dynamic dispatch: `send(:binding)`,
/// `self.public_send(:binding)`, `method(:binding).call`, a block
/// argument `&method(:binding)`, and a `send` or `method` whose method
/// name is only known at run time.
const PINION_CACHE: &str = "json.partial!(@pinion, as: :pinion, cache: true)\n";
const PINION_PARTIAL: &str =
    "json.id pinion.id\njson.fresh send(:binding).local_variable_get(:cache)\n";
const RATCHET_CACHE: &str = "json.partial!(@ratchet, as: :ratchet, cache: true)\n";
const RATCHET_PARTIAL: &str =
    "json.id ratchet.id\njson.fresh self.public_send(:binding).local_variable_get(:cache)\n";
const LEVER_CACHE: &str = "json.partial!(@lever, as: :lever, cache: true)\n";
const LEVER_PARTIAL: &str =
    "json.id lever.id\njson.fresh method(:binding).call.local_variable_get(:cache)\n";
const SPINDLE_CACHE: &str = "json.partial!(@spindle, as: :spindle, cache: true)\n";
const SPINDLE_PARTIAL: &str =
    "json.id spindle.id\njson.fresh __send__(@reader).local_variable_get(:cache)\n";
const CAM_CACHE: &str = "json.partial!(@cam, as: :cam, cache: true)\n";
const CAM_PARTIAL: &str =
    "json.id cam.id\njson.fresh proc(&method(:binding)).call.local_variable_get(:cache)\n";
const PAWL_CACHE: &str = "json.partial!(@pawl, as: :pawl, cache: true)\n";
const PAWL_PARTIAL: &str =
    "json.id pawl.id\njson.fresh method(@reader).call.local_variable_get(:cache)\n";

/// `layout:` is an Action View render option, not a local: Rails looks
/// for `widgets/_frame` and raises when it is missing.
const LAYOUT_OPTION: &str = "json.partial!(@widget, as: :widget, layout: \"widgets/frame\")\n";

/// A local whose value is not a literal.
const DYNAMIC_CACHE: &str = "json.partial!(@widget, as: :widget, cache: @flag)\n";

/// A `partial:` whose value is not a literal: Ruby calls `partial_for`
/// before the render, and a call that passes only the record drops it.
const DYNAMIC_PARTIAL: &str =
    "json.partial!(@widget, partial: partial_for(@widget), as: :widget)\n";

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
        ("app/models/gizmo.rb", "class Gizmo < ApplicationRecord\nend\n"),
        ("app/models/sprocket.rb", "class Sprocket < ApplicationRecord\nend\n"),
        ("app/models/cog.rb", "class Cog < ApplicationRecord\nend\n"),
        ("app/models/pinion.rb", "class Pinion < ApplicationRecord\nend\n"),
        ("app/models/ratchet.rb", "class Ratchet < ApplicationRecord\nend\n"),
        ("app/models/lever.rb", "class Lever < ApplicationRecord\nend\n"),
        ("app/models/spindle.rb", "class Spindle < ApplicationRecord\nend\n"),
        ("app/models/cam.rb", "class Cam < ApplicationRecord\nend\n"),
        ("app/models/pawl.rb", "class Pawl < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("app/views/widgets/_widget.json.jbuilder", PARTIAL),
        ("app/views/widgets/detail.json.jbuilder", DETAIL),
        ("app/views/widgets/wrapped.json.jbuilder", WRAPPED),
        ("app/views/widgets/aliased.json.jbuilder", ALIASED),
        ("app/views/widgets/cached.json.jbuilder", CACHED),
        ("app/views/widgets/aliased_cached.json.jbuilder", ALIASED_CACHED),
        ("app/views/widgets/stray.json.jbuilder", STRAY),
        ("app/views/widgets/extra.json.jbuilder", EXTRA),
        ("app/views/widgets/unknown.json.jbuilder", UNKNOWN),
        ("app/views/widgets/entry.json.jbuilder", ENTRY),
        ("app/views/widgets/entry_string.json.jbuilder", ENTRY_STRING),
        ("app/views/widgets/widget_string.json.jbuilder", WIDGET_STRING),
        ("app/views/widgets/dynamic_as.json.jbuilder", DYNAMIC_AS),
        ("app/views/widgets/cache_option.json.jbuilder", CACHE_OPTION),
        ("app/views/widgets/other_path.json.jbuilder", OTHER_PATH),
        ("app/views/widgets/items_cache.json.jbuilder", ITEMS_CACHE),
        ("app/views/widgets/items_cached.json.jbuilder", ITEMS_CACHED),
        ("app/views/widgets/gizmo_cache.json.jbuilder", GIZMO_CACHE),
        ("app/views/gizmos/_gizmo.json.jbuilder", GIZMO_PARTIAL),
        ("app/views/widgets/sprocket_cache.json.jbuilder", SPROCKET_CACHE),
        ("app/views/sprockets/_sprocket.json.jbuilder", SPROCKET_PARTIAL),
        ("app/views/widgets/cog_cache.json.jbuilder", COG_CACHE),
        ("app/views/cogs/_cog.json.jbuilder", COG_PARTIAL),
        ("app/views/widgets/layout_option.json.jbuilder", LAYOUT_OPTION),
        ("app/views/widgets/dynamic_cache.json.jbuilder", DYNAMIC_CACHE),
        ("app/views/widgets/dynamic_partial.json.jbuilder", DYNAMIC_PARTIAL),
        ("app/views/widgets/pinion_cache.json.jbuilder", PINION_CACHE),
        ("app/views/pinions/_pinion.json.jbuilder", PINION_PARTIAL),
        ("app/views/widgets/ratchet_cache.json.jbuilder", RATCHET_CACHE),
        ("app/views/ratchets/_ratchet.json.jbuilder", RATCHET_PARTIAL),
        ("app/views/widgets/lever_cache.json.jbuilder", LEVER_CACHE),
        ("app/views/levers/_lever.json.jbuilder", LEVER_PARTIAL),
        ("app/views/widgets/spindle_cache.json.jbuilder", SPINDLE_CACHE),
        ("app/views/spindles/_spindle.json.jbuilder", SPINDLE_PARTIAL),
        ("app/views/widgets/cam_cache.json.jbuilder", CAM_CACHE),
        ("app/views/cams/_cam.json.jbuilder", CAM_PARTIAL),
        ("app/views/widgets/pawl_cache.json.jbuilder", PAWL_CACHE),
        ("app/views/pawls/_pawl.json.jbuilder", PAWL_PARTIAL),
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
        let errors: Vec<String> = result.errors().map(|e| String::from_utf8_lossy(e.message()).into_owned()).collect();
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
    for template in ["cached", "aliased_cached"] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            src.contains("io << Views::Widgets.widget_json(widget)"),
            "{template}: `cached:` is a rendering option, not a local:\n{src}"
        );
    }
    let src = view(&files, "widgets/stray_json.rb");
    assert!(
        src.contains("io << Views::Widgets.widget_json(gadget)"),
        "the record's class picks the partial, not the ivar's name:\n{src}"
    );
}

/// An `as:` other than `widget` names a local the lowered partial does
/// not take its record under: refused with the reason, rather than
/// calling a partial that would miss its local or rendering nothing.
#[test]
fn another_as_is_refused() {
    let files = emitted();
    let src = view(&files, "widgets/entry_json.rb");
    assert!(
        !src.contains("Views::Widgets.") && src.contains("partial! of a record not supported"),
        "entry: refused with its reason:\n{src}"
    );
}

/// A literal local the record's partial never reads (`label: "x"`,
/// `cache: true`) changes nothing Rails renders, so the call renders
/// the partial as it does without it. `cached: true` was already taken
/// out of the locals. A `partial:` that names another path is
/// overwritten by the record. The same holds per element of a key's
/// collection.
#[test]
fn a_record_with_an_unread_literal_local_renders_its_partial() {
    let files = emitted();
    for template in ["extra", "cache_option", "other_path"] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            src.contains("io << Views::Widgets.widget_json(widget)") && !src.contains("not supported"),
            "{template}: the record's partial, with the record only:\n{src}"
        );
    }
    for template in ["items_cache", "items_cached"] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            src.contains("Views::Widgets.widget_json(w)") && !src.contains("not supported"),
            "{template}: each element's partial:\n{src}"
        );
    }
}

/// A local the partial reads is refused with the reason: the lowered
/// partial has no parameter for it, and its output depends on it. An
/// Action View render option (`layout:`) and a non-literal value stay
/// unrecognised, refused where they stand (#1539).
#[test]
fn a_record_with_a_local_it_reads_or_a_render_option_is_refused() {
    roundhouse::ingest::survey::activate();
    let files = emitted();
    let gaps = roundhouse::ingest::survey::drain();
    let src = view(&files, "widgets/gizmo_cache_json.rb");
    assert!(
        !src.contains("Views::Gizmos.") && src.contains("partial! of a record not supported"),
        "gizmo_cache: refused:\n{src}"
    );
    assert!(
        gaps.iter().any(|g| g.to_string().contains("`gizmos/gizmo` reads the local `cache`")),
        "gizmo_cache: the gap names the local the partial reads: {gaps:?}"
    );
    for template in ["layout_option", "dynamic_cache"] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            !src.contains("Views::Widgets.")
                && !src.contains("io << \"\"")
                && src.contains("jbuilder call with unrecognised arguments not supported"),
            "{template}: no partial call is emitted, and the statement is refused:\n{src}"
        );
    }
}

/// A partial that reads its locals through `binding` may read the one
/// the call passes without naming it, with or without a `self.`
/// receiver, so both calls are refused as `gizmo_cache` is.
#[test]
fn a_record_whose_partial_reads_locals_through_binding_is_refused() {
    roundhouse::ingest::survey::activate();
    let files = emitted();
    let gaps = roundhouse::ingest::survey::drain();
    for (template, partial) in [("sprocket_cache", "sprockets/sprocket"), ("cog_cache", "cogs/cog")] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            !src.contains("Views::Sprockets.")
                && !src.contains("Views::Cogs.")
                && src.contains("partial! of a record not supported"),
            "{template}: refused:\n{src}"
        );
        assert!(
            gaps.iter().any(|g| g.to_string().contains(&format!("`{partial}` reads the local `cache`"))),
            "{template}: the gap names the local the partial may read: {gaps:?}"
        );
    }
}

/// The same read through `send`, `public_send`, `method`, a block
/// argument `&method(:binding)`, and a `__send__` or `method` of a name
/// only known at run time: each may reach `binding`, so each call is
/// refused as `sprocket_cache` is.
#[test]
fn a_record_whose_partial_reads_locals_through_dispatch_is_refused() {
    roundhouse::ingest::survey::activate();
    let files = emitted();
    let gaps = roundhouse::ingest::survey::drain();
    for (template, partial, module) in [
        ("pinion_cache", "pinions/pinion", "Views::Pinions."),
        ("ratchet_cache", "ratchets/ratchet", "Views::Ratchets."),
        ("lever_cache", "levers/lever", "Views::Levers."),
        ("spindle_cache", "spindles/spindle", "Views::Spindles."),
        ("cam_cache", "cams/cam", "Views::Cams."),
        ("pawl_cache", "pawls/pawl", "Views::Pawls."),
    ] {
        let src = view(&files, &format!("widgets/{template}_json.rb"));
        assert!(
            !src.contains(module) && src.contains("partial! of a record not supported"),
            "{template}: refused:\n{src}"
        );
        assert!(
            gaps.iter().any(|g| g.to_string().contains(&format!("`{partial}` reads the local `cache`"))),
            "{template}: the gap names the local the partial may read: {gaps:?}"
        );
    }
}

/// A `partial:` that is not a literal is evaluated before the render,
/// so the record call is not lowered and is refused where it stands,
/// as a non-literal local is (#1616).
#[test]
fn a_record_with_a_non_literal_partial_option_is_refused() {
    let files = emitted();
    let src = view(&files, "widgets/dynamic_partial_json.rb");
    assert!(
        !src.contains("Views::Widgets.")
            && !src.contains("io << \"\"")
            && src.contains("jbuilder call with unrecognised arguments not supported"),
        "dynamic_partial: no partial call is emitted, and the statement is refused:\n{src}"
    );
}

/// A record whose class is not statically known has no partial to
/// name. It is refused where it stands, not written as nothing: an
/// empty object there was a 200 with the record's keys missing.
#[test]
fn a_record_of_no_known_model_is_refused_not_rendered_empty() {
    let files = emitted();
    let src = view(&files, "widgets/unknown_json.rb");
    assert!(
        !src.contains("Views::Widgets.")
            && !src.contains("io << \"\"")
            && src.contains("partial! of a record not supported"),
        "unknown: refused with its reason:\n{src}"
    );
}

/// A String `as:` is compared like the Symbol: `as: "entry"` is
/// refused as `as: :entry` is, rather than binding the record as
/// `widget` for a partial that reads `entry`; `as: "widget"` renders
/// the partial. An `as:` that is not a literal is not lowered, and is
/// refused where it stands (#1539).
#[test]
fn a_string_as_is_compared_like_a_symbol() {
    let files = emitted();
    let src = view(&files, "widgets/entry_string_json.rb");
    assert!(
        !src.contains("Views::Widgets.") && src.contains("partial! of a record not supported"),
        "entry_string: refused with its reason:\n{src}"
    );
    let src = view(&files, "widgets/dynamic_as_json.rb");
    assert!(
        !src.contains("Views::Widgets.")
            && !src.contains("io << \"\"")
            && src.contains("jbuilder call with unrecognised arguments not supported"),
        "dynamic_as: no partial call is emitted, and the statement is refused:\n{src}"
    );
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
    for name in [
        "_widget_json.rb",
        "detail_json.rb",
        "wrapped_json.rb",
        "aliased_json.rb",
        "cached_json.rb",
        "extra_json.rb",
        "cache_option_json.rb",
        "other_path_json.rb",
        "items_cache_json.rb",
        "items_cached_json.rb",
    ] {
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
widgets = [widget, Widget.new(2, "c", 7)]
puts JSON.generate(
  "detail" => JSON.parse(Views::Widgets.detail_json(widget)),
  "wrapped" => JSON.parse(Views::Widgets.wrapped_json(widget)),
  "aliased" => JSON.parse(Views::Widgets.aliased_json(widget)),
  "cached" => JSON.parse(Views::Widgets.cached_json(widget)),
  "extra" => JSON.parse(Views::Widgets.extra_json(widget)),
  "cache_option" => JSON.parse(Views::Widgets.cache_option_json(widget)),
  "other_path" => JSON.parse(Views::Widgets.other_path_json(widget)),
  "items_cache" => JSON.parse(Views::Widgets.items_cache_json(widgets)),
  "items_cached" => JSON.parse(Views::Widgets.items_cached_json(widgets)),
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
        r#""aliased":{"id":1,"name":"b"},"#,
        r#""cached":{"id":1,"name":"b"},"#,
        r#""extra":{"id":1,"name":"b"},"#,
        r#""cache_option":{"id":1,"name":"b"},"#,
        r#""other_path":{"id":1,"name":"b"},"#,
        r#""items_cache":{"items":[{"id":1,"name":"b"},{"id":2,"name":"c"}]},"#,
        r#""items_cached":{"items":[{"id":1,"name":"b"},{"id":2,"name":"c"}]}}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
