//! A `json.<key>(…)` call the jbuilder lowering does not recognise
//! (#1539).
//!
//! The classifier tagged such a statement `Unknown` and the walker wrote
//! it as nothing: the key was missing from the response, its arguments
//! were never evaluated, and nothing reported it. Jbuilder renders
//! every one of these shapes, or raises. A call on `json` the lowering
//! cannot follow is now refused where it stands, the way a dynamic
//! `extract!` is: the survey lists it and the emitted view raises there.
//!
//! `json.set!(K, …)` with a literal key is Jbuilder's own spelling of
//! `json.K(…)` (an undefined method on the builder is sent to `set!`),
//! and lowers as that.
//!
//! The lowering is shared by every target, so the survey check covers
//! them all; the emitted Ruby (the `ruby` and `spinel` source) is also
//! run on CRuby next to the runtime's `JsonBuilder`, with plain Structs
//! for records. Expected values are what ActionView 8.1.4 with jbuilder
//! 2.15.1 render for the same templates and the rows `(1, "a")` and
//! `(2, "b")`.

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

const PARTIAL: &str = "json.id widget.id\njson.name widget.name\n";

/// The templates, by action. Each sets `count` before the statement
/// under test, so a dropped statement still renders a 200.
const TEMPLATES: &[(&str, &str)] = &[
    // A trailing option that is a call: jbuilder evaluates it and, with
    // a block, ignores it. The lowering does not evaluate it, so it
    // refuses rather than build the array without it.
    (
        "options_call",
        "json.count @widgets.size\njson.widgets(@widgets, cache: @widgets.size) do |widget|\n  json.label widget.name\nend\n",
    ),
    // The same, where the call raises in jbuilder.
    (
        "options_raise",
        "json.count @widgets.size\njson.widgets(@widgets, cache: raise(\"bad\")) do |widget|\n  json.label widget.name\nend\n",
    ),
    // Partial options in a variable: only known at run time.
    (
        "opts_var",
        "opts = { partial: \"widgets/widget\", as: :widget }\njson.count @widgets.size\njson.widgets @widgets, opts\n",
    ),
    (
        "opts_var_block",
        "opts = { partial: \"widgets/widget\", as: :widget }\njson.count @widgets.size\njson.widgets(@widgets, opts) do |widget|\n  json.label widget.name\nend\n",
    ),
    // `set!` with no value and a block that names a parameter: jbuilder
    // yields the builder to it (`_merge_block`), and its calls go
    // through `g`, not `json`.
    (
        "set_block_param",
        "json.count @widgets.size\njson.set!(:gadget) do |g|\n  g.name \"g\"\nend\n",
    ),
    // An inline partial whose `cached:` is a call: jbuilder evaluates
    // it (and raises `RuntimeError: bad` here); the lowered call to the
    // partial would not. Both spellings.
    (
        "partial_cached_call",
        "json.count @widgets.size\njson.gadget @widgets.first, partial: \"widgets/widget\", as: :widget, cached: raise(\"bad\")\n",
    ),
    (
        "set_partial_cached_call",
        "json.count @widgets.size\njson.set! :gadget, @widgets.first, partial: \"widgets/widget\", as: :widget, cached: raise(\"bad\")\n",
    ),
    // Any other key is a local jbuilder hands the partial; the lowered
    // call passes only the record.
    (
        "partial_local",
        "json.count @widgets.size\njson.gadget @widgets.first, partial: \"widgets/widget\", as: :widget, label: @widgets.size\n",
    ),
];

/// An inline partial on one record or on a collection, in both
/// spellings, and what jbuilder renders for each: an object for the
/// record, one object per element for the collection.
const INLINE_PARTIAL: &[(&str, &str, &str)] = &[
    (
        "partial_record",
        "json.count @widgets.size\njson.gadget @widgets.first, partial: \"widgets/widget\", as: :widget\n",
        r#"{"count":2,"gadget":{"id":1,"name":"a"}}"#,
    ),
    (
        "set_partial_record",
        "json.count @widgets.size\njson.set! :gadget, @widgets.first, partial: \"widgets/widget\", as: :widget\n",
        r#"{"count":2,"gadget":{"id":1,"name":"a"}}"#,
    ),
    (
        "partial_collection",
        "json.count @widgets.size\njson.gadgets @widgets, partial: \"widgets/widget\", as: :widget\n",
        r#"{"count":2,"gadgets":[{"id":1,"name":"a"},{"id":2,"name":"b"}]}"#,
    ),
    (
        "set_partial_collection",
        "json.count @widgets.size\njson.set! :gadgets, @widgets, partial: \"widgets/widget\", as: :widget\n",
        r#"{"count":2,"gadgets":[{"id":1,"name":"a"},{"id":2,"name":"b"}]}"#,
    ),
    // A nil positional: jbuilder's `_set_inline_partial` answers `[]`
    // without calling the partial.
    (
        "partial_nil",
        "json.count @widgets.size\njson.gadgets nil, partial: \"widgets/widget\", as: :widget\n",
        r#"{"count":2,"gadgets":[]}"#,
    ),
    (
        "set_partial_nil",
        "json.count @widgets.size\njson.set! :gadgets, nil, partial: \"widgets/widget\", as: :widget\n",
        r#"{"count":2,"gadgets":[]}"#,
    ),
    // A record read that is nil at run time.
    (
        "set_partial_missing_record",
        "json.count @widgets.size\njson.set! :gadget, @widgets[2], partial: \"widgets/widget\", as: :widget\n",
        r#"{"count":2,"gadget":[]}"#,
    ),
];

/// `json.set!` with a literal key, which lowers as `json.<key>` does,
/// and what jbuilder renders for each.
const SET_BANG: &[(&str, &str, &str)] = &[
    (
        "set_pair",
        "json.count @widgets.size\njson.set!(:gadget, @widgets.size)\n",
        r#"{"count":2,"gadget":2}"#,
    ),
    (
        "set_collection",
        "json.count @widgets.size\njson.set!(:gadgets, @widgets) do |widget|\n  json.label widget.name\nend\n",
        r#"{"count":2,"gadgets":[{"label":"a"},{"label":"b"}]}"#,
    ),
    (
        "set_block",
        "json.count @widgets.size\njson.set!(:gadget) do\n  json.name \"g\"\nend\n",
        r#"{"count":2,"gadget":{"name":"g"}}"#,
    ),
    // A String key is the same key.
    (
        "set_string_key",
        "json.count @widgets.size\njson.set!(\"gadget\", @widgets.size)\n",
        r#"{"count":2,"gadget":2}"#,
    ),
];

/// The construct the refusal names, as the Ruby emit writes it.
const REFUSAL: &str = "jbuilder call with unrecognised arguments not supported";

fn app() -> roundhouse::App {
    let mut controller = String::from("class WidgetsController < ApplicationController\n");
    let mut routes = String::from("Rails.application.routes.draw do\n");
    let mut files: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    let all = TEMPLATES
        .iter()
        .copied()
        .chain(SET_BANG.iter().chain(INLINE_PARTIAL).map(|(n, t, _)| (*n, *t)));
    for (name, template) in all {
        controller.push_str(&format!("  def {name}\n    @widgets = Widget.all\n    render :{name}\n  end\n\n"));
        routes.push_str(&format!(
            "  get \"{name}\", to: \"widgets#{name}\", defaults: {{ format: :json }}\n"
        ));
        files.insert(
            PathBuf::from(format!("app/views/widgets/{name}.json.jbuilder")),
            template.as_bytes().to_vec(),
        );
    }
    controller.push_str("end\n");
    routes.push_str("end\n");
    for (path, source) in [
        ("db/schema.rb", SCHEMA),
        ("config/routes.rb", routes.as_str()),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", controller.as_str()),
        ("app/views/widgets/_widget.json.jbuilder", PARTIAL),
    ] {
        files.insert(PathBuf::from(path), source.as_bytes().to_vec());
    }
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

fn emitted() -> Vec<(String, String)> {
    ruby::emit_lowered_jbuilder_views(&app())
        .into_iter()
        .map(|f| (f.path.to_string_lossy().into_owned(), f.content))
        .collect()
}

fn view<'a>(files: &'a [(String, String)], name: &str) -> &'a str {
    let suffix = format!("widgets/{name}_json.rb");
    files
        .iter()
        .find(|(p, _)| p.ends_with(&suffix))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| panic!("no emitted view ends with {suffix}"))
}

/// Each template on CRuby with two widgets: its JSON, or the class and
/// message of what it raises.
fn render_all(tag: &str, files: &[(String, String)]) -> HashMap<String, String> {
    let dir = std::env::temp_dir().join(format!("roundhouse-jbuilder-unrecognised-call-{tag}-{}", std::process::id()));
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let mut script = format!("require {:?}\nrequire \"json\"\n", runtime.display().to_string());
    script.push_str(&format!(
        "require {:?}\n",
        dir.join("app/views/widgets/_widget_json.rb").display().to_string()
    ));
    script.push_str("Widget = Struct.new(:id, :name)\nwidgets = [Widget.new(1, \"a\"), Widget.new(2, \"b\")]\n");
    for name in TEMPLATES.iter().map(|(n, _)| *n).chain(SET_BANG.iter().chain(INLINE_PARTIAL).map(|(n, _, _)| *n)) {
        let file = dir.join(format!("app/views/widgets/{name}_json.rb"));
        script.push_str(&format!(
            "require {file:?}\nout = begin\n  Views::Widgets.{name}_json(widgets)\nrescue => e\n  \"raises #{{e.class}}: #{{e.message}}\"\nend\nputs \"{name}\\t#{{out}}\"\n",
            file = file.display().to_string()
        ));
    }
    let output = Command::new("ruby").args(["-e", &script]).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn every_emitted_view_parses() {
    for (path, source) in emitted().iter().filter(|(p, _)| p.ends_with(".rb")) {
        let result = ruby_prism::parse(source.as_bytes());
        let errors: Vec<String> = result.errors().map(|e| String::from_utf8_lossy(e.message()).into_owned()).collect();
        assert!(errors.is_empty(), "{path} does not parse: {errors:?}\n{source}");
    }
}

/// The lowering every target shares records one gap per refused
/// statement, so `roundhouse check` and `--allow-unsupported` report it.
#[test]
fn an_unrecognised_json_call_is_a_survey_gap() {
    roundhouse::ingest::survey::activate();
    let _ = emitted();
    let gaps: Vec<String> = roundhouse::ingest::survey::drain()
        .iter()
        .map(|e| e.to_string())
        .filter(|m| m.contains("does not recognise"))
        .collect();
    assert_eq!(gaps.len(), TEMPLATES.len(), "one gap per template:\n{gaps:#?}");
}

/// The emitted view raises at the statement, after the pair before it,
/// instead of writing nothing for it.
#[test]
fn an_unrecognised_json_call_is_refused_where_it_stands() {
    let files = emitted();
    for (name, _) in TEMPLATES {
        let src = view(&files, name);
        assert_eq!(src.matches(REFUSAL).count(), 1, "{name}: one refusal:\n{src}");
        assert!(!src.contains("io << \"\"\n"), "{name}: nothing is written as an empty append:\n{src}");
        let count = src.find("\\\"count\\\":").unwrap_or_else(|| panic!("{name}: the count pair:\n{src}"));
        assert!(count < src.find(REFUSAL).unwrap(), "{name}: the refusal stands after `count`:\n{src}");
    }
}

/// On CRuby each template raises the refusal. Jbuilder raises
/// `RuntimeError: bad` on `options_raise` and the two `cached:` calls
/// and renders the others; before
/// the fix the first four rendered `{"count":2}` and `set_block_param`
/// raised `NoMethodError` mapping over the Symbol `:gadget`.
#[test]
fn an_unrecognised_json_call_raises_when_rendered() {
    let out = render_all("refused", &emitted());
    for (name, _) in TEMPLATES {
        let got = &out[*name];
        assert!(
            got.starts_with("raises RuntimeError: ") && got.contains(REFUSAL),
            "{name}: {got}"
        );
    }
}

/// `json.set!(:k, …)` is `json.k(…)`: a pair, an array built by the
/// block, a nested object. Before, each of these wrote nothing.
#[test]
fn set_bang_with_a_literal_key_lowers_as_the_key() {
    let files = emitted();
    for (name, _, _) in SET_BANG {
        let src = view(&files, name);
        assert!(!src.contains(REFUSAL), "{name}: lowered, not refused:\n{src}");
        assert!(!src.contains("\\\"set!\\\":"), "{name}: `set!` is not the key:\n{src}");
    }
    let pair = view(&files, "set_pair");
    assert!(
        pair.contains("io << \"\\\"gadget\\\":\"\n      io << JsonBuilder.encode_value(widgets.size)\n"),
        "a pair:\n{pair}"
    );
    let collection = view(&files, "set_collection");
    assert!(
        collection.contains("__col0 = widgets") && collection.contains("__col0.map"),
        "an array over the collection, not over the key:\n{collection}"
    );
}

/// On CRuby the `set!` templates render what jbuilder 2.15.1 renders.
#[test]
fn set_bang_with_a_literal_key_renders_what_jbuilder_renders() {
    let out = render_all("set_bang", &emitted());
    for (name, _, expected) in SET_BANG {
        assert_eq!(&out[*name], expected, "{name}");
    }
}

/// `json.<key> col, partial: P, as: V` and its `set!` spelling render
/// the partial once per element when the positional is a collection,
/// and once when it is a record. Before, the collection was passed to
/// one call of the partial, which raised `NoMethodError` reading `id`
/// on the Array.
#[test]
fn an_inline_partial_pair_renders_what_jbuilder_renders() {
    let files = emitted();
    for name in ["partial_collection", "set_partial_collection"] {
        let src = view(&files, name);
        assert!(
            src.contains("widgets.map { |widget| Views::Widgets.widget_json(widget) }.join(\",\")"),
            "{name}: one partial call per element:\n{src}"
        );
    }
    let out = render_all("inline_partial", &files);
    for (name, _, expected) in INLINE_PARTIAL {
        assert_eq!(&out[*name], expected, "{name}");
    }
}
