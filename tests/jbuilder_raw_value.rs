//! `raw(x)` in a jbuilder value.
//!
//! ```ruby
//! json.labels [widget.name, raw(widget.note)]
//! ```
//!
//! `raw` only marks a String html_safe, which JSON encoding ignores:
//! Rails renders the same String. The jbuilder lowering left the call,
//! and the view module it landed in has no `raw` ("undefined method
//! 'raw' for an instance of Views::Widgets"). It is now Rails' `x.to_s`
//! (`raw(nil)` renders `""`), in any value position (inside an Array, a
//! conditional, a call argument), and `h(raw(x))` is that String
//! unescaped, as `h` leaves an html_safe String alone. `h(x)` keeps
//! escaping everywhere else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const PARTIAL: &str = r#"json.title raw(widget.name)
json.labels [widget.name, raw(widget.note)]
json.marked(widget.note ? raw(widget.note.upcase) : "none")
json.escaped h(widget.name)
json.bare raw(widget.note)
json.kept h(raw(widget.name))
"#;

fn emitted() -> Vec<(String, String)> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.string \"note\"\n  end\nend\n",
        ),
        ("config/routes.rb", "Rails.application.routes.draw do\n  get \"widgets/:id\", to: \"widgets#show\", defaults: { format: :json }\nend\n"),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        (
            "app/controllers/widgets_controller.rb",
            "class WidgetsController < ApplicationController\n  def show\n    @widget = Widget.find(params[:id])\n    render :show\n  end\nend\n",
        ),
        ("app/views/widgets/show.json.jbuilder", "json.partial! @widget\n"),
        ("app/views/widgets/_widget.json.jbuilder", PARTIAL),
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

fn partial(files: &[(String, String)]) -> &str {
    files
        .iter()
        .find(|(p, _)| p.ends_with("widgets/_widget_json.rb"))
        .map(|(_, c)| c.as_str())
        .expect("the partial is emitted")
}

#[test]
fn raw_is_the_value_itself() {
    let files = emitted();
    let src = partial(&files);
    assert!(!src.contains("raw("), "{src}");
    assert!(!src.contains("__rh_raw"), "{src}");
    assert!(src.contains("[widget.name, widget.note.to_s]"), "{src}");
    // `h` leaves an html_safe String alone, so `h(raw(x))` is not escaped.
    assert!(!src.contains("html_escape(widget.name.to_s)"), "{src}");
    assert!(src.contains("ActionView::ViewHelpers.html_escape(widget.name)"), "{src}");
}

/// The partial on CRuby with the runtime's `JsonBuilder` and view
/// helpers, against what jbuilder renders for the same row.
#[test]
fn the_partial_renders_what_jbuilder_renders() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!("roundhouse-jbuilder-raw-{}", std::process::id()));
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let partial = dir.join("app/views/widgets/_widget_json.rb");
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby");
    let script = format!(
        r#"require {json_builder:?}
module ActionView; module ViewHelpers; def self.html_escape(s) = s.to_s.gsub("<", "&lt;").gsub(">", "&gt;"); end; end
require {partial:?}
require "json"
Widget = Struct.new(:name, :note)
puts Views::Widgets.widget_json(Widget.new("<a>", "hi"))
puts Views::Widgets.widget_json(Widget.new("b", nil))
"#,
        json_builder = root.join("json_builder.rb").display().to_string(),
        partial = partial.display().to_string(),
    );
    let output = Command::new("ruby").args(["-e", &script]).output().unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // Jbuilder 2.15 on Rails 8.1 for the same rows: `raw(x)` is
    // `x.to_s.html_safe`, so `raw(nil)` is `""`, and `h` leaves it as is.
    // Rails' JSON encoding writes `<`, `>` and `&` as `\u003c`, `\u003e`
    // and `\u0026` (`escape_html_entities_in_json`), and so does the
    // runtime: the text is compared as written.
    let expected = concat!(
        r#"{"title":"\u003ca\u003e","labels":["\u003ca\u003e","hi"],"marked":"HI","#,
        r#""escaped":"\u0026lt;a\u0026gt;","bare":"hi","kept":"\u003ca\u003e"}"#,
        "\n",
        r#"{"title":"b","labels":["b",""],"marked":"none","escaped":"b","bare":"","kept":"b"}"#,
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), expected);
}
