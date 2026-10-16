//! An Array as a jbuilder value: `json.tags([widget.name, nil])`.
//!
//! `JsonBuilder.encode_value` had no Array branch and fell through to
//! "stringify and quote", so the value came out as the Array's `to_s`
//! (`"[\"b\", nil]"`) where Jbuilder writes a JSON array (`["b",null]`).
//! Each element is now encoded the way a scalar value is, and a Hash
//! element is a JSON object (String keys, values encoded the same way),
//! as Jbuilder writes `json.tags([{name: "b"}, [1, nil]])`.
//! The Spinel side is `jbuilder_array_value_spinel.rs`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::emit::ruby;
use roundhouse::ingest::ingest_app_from_tree;

const PARTIAL: &str = r#"json.tags([widget.name, nil])
json.nested [[widget.size, 1.5], [], "x"]
json.mixed([{name: widget.name}, [1, nil]])
"#;

fn emitted() -> Vec<(String, String)> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n    t.integer \"size\"\n  end\nend\n",
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

/// The partial on CRuby with the runtime's `JsonBuilder`, against what
/// jbuilder renders for the same row. The output is compared as text, so
/// a JSON-in-a-string value cannot pass for an array.
#[test]
fn an_array_value_is_a_json_array() {
    let files = emitted();
    let dir = std::env::temp_dir().join(format!("roundhouse-jbuilder-array-{}", std::process::id()));
    for (path, source) in files.iter().filter(|(p, _)| p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let partial = dir.join("app/views/widgets/_widget_json.rb");
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r#"require {runtime:?}
require {partial:?}
Widget = Struct.new(:name, :size)
puts Views::Widgets.widget_json(Widget.new("b", 2))
puts Views::Widgets.widget_json(Widget.new(nil, nil))
"#,
        runtime = runtime.display().to_string(),
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
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        concat!(
            r#"{"tags":["b",null],"nested":[[2,1.5],[],"x"],"mixed":[{"name":"b"},[1,null]]}"#,
            "\n",
            r#"{"tags":[null,null],"nested":[[null,1.5],[],"x"],"mixed":[{"name":null},[1,null]]}"#,
        )
    );
}
