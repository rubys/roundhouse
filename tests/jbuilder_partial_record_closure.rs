//! A record partial (`json.partial! @widget`, with or without
//! `cached: true`) that reads the renderer's ivars and the request's
//! `params`.
//!
//! The render graph walks each template as written, where the record is
//! an ivar, and it only knew a bare local as the record. The template's
//! ivar closure therefore missed the partial's ivars. The partial itself
//! took them as parameters, and the template's call passed them under
//! names it never defined: `widget_json(widget, hide_owner)` inside
//! `def self.show_json(widget)`, a NameError on every render.
//!
//! The request's `params` read in a view had no parameter at all, so it
//! stayed a bare method call no view module defines. It is now one more
//! ivar of the closure, the controller's `@params`, typed as the
//! String-keyed request Hash so `params[:q]` reads `"q"`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const ROUTES: &str = r#"Rails.application.routes.draw do
  get "widgets/:id", to: "widgets#show", defaults: { format: :json }
  get "widgets/:id/plain", to: "widgets#plain", defaults: { format: :json }
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def show
    @widget = Widget.find(params[:id])
    render :show
  end

  def plain
    @widget = Widget.find(params[:id])
    @hide_owner = true
    render :plain
  end
end
"#;

const PARTIAL: &str = r#"json.name widget.name
json.owner "x" unless @hide_owner
json.q params[:q]
"#;

fn emitted(target: BuildTarget) -> Vec<(String, String)> {
    let files: HashMap<PathBuf, Vec<u8>> = [
        (
            "db/schema.rb",
            "ActiveRecord::Schema.define do\n  create_table \"widgets\", force: :cascade do |t|\n    t.string \"name\"\n  end\nend\n",
        ),
        ("config/routes.rb", ROUTES),
        ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n"),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\nend\n"),
        ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
        ("app/controllers/widgets_controller.rb", CONTROLLER),
        ("app/views/widgets/show.json.jbuilder", "json.partial!(@widget, cached: true)\n"),
        ("app/views/widgets/plain.json.jbuilder", "json.partial! @widget\n"),
        ("app/views/widgets/_widget.json.jbuilder", PARTIAL),
    ]
    .iter()
    .map(|(p, c)| (PathBuf::from(p), c.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(files).expect("ingest");
    roundhouse::session::analyze_and_lower(&mut app);
    target_files(&app, Path::new("fixtures/tiny-blog"), target).expect("emit")
}

fn file<'a>(files: &'a [(String, String)], suffix: &str) -> &'a str {
    files
        .iter()
        .find(|(p, _)| p.ends_with(suffix))
        .map(|(_, c)| c.as_str())
        .unwrap_or_else(|| panic!("no emitted file ends with {suffix}"))
}

#[test]
fn the_template_takes_what_its_partial_reads_and_the_controller_passes_it() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = emitted(target);
        for name in ["show", "plain"] {
            let src = file(&files, &format!("widgets/{name}_json.rb"));
            assert!(src.contains(&format!("def self.{name}_json(widget, hide_owner, params)")), "{target:?}:\n{src}");
            assert!(src.contains("Views::Widgets.widget_json(widget, hide_owner, params)"), "{target:?}:\n{src}");
            let controller = file(&files, "widgets_controller.rb");
            assert!(
                controller.contains(&format!("Views::Widgets.{name}_json(@widget, @hide_owner, @params)")),
                "{target:?}:\n{controller}"
            );
        }
        let partial = file(&files, "widgets/_widget_json.rb");
        assert!(partial.contains("def self.widget_json(widget, hide_owner, params)"), "{target:?}:\n{partial}");
        assert!(partial.contains("params[\"q\"]"), "{target:?}:\n{partial}");
    }
}

/// The emitted templates on CRuby with the runtime's `JsonBuilder`,
/// against what jbuilder renders for the same row, ivars and params.
#[test]
fn the_templates_render_what_jbuilder_renders() {
    let files = emitted(BuildTarget::Ruby);
    let dir = std::env::temp_dir().join(format!("roundhouse-jbuilder-record-closure-{}", std::process::id()));
    for (path, source) in files.iter().filter(|(p, _)| p.starts_with("app/views/widgets/") && p.ends_with(".rb")) {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
    }
    std::fs::write(dir.join("app/views.rb"), "").unwrap();
    let mut requires = String::new();
    for name in ["_widget_json.rb", "show_json.rb", "plain_json.rb"] {
        let file = dir.join("app/views/widgets").join(name);
        requires.push_str(&format!("require {:?}\n", file.display().to_string()));
    }
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/json_builder.rb");
    let script = format!(
        r#"require {runtime:?}
{requires}
require "json"
Widget = Struct.new(:id, :name)
widget = Widget.new(1, "b")
puts JSON.generate([
  JSON.parse(Views::Widgets.show_json(widget, nil, {{ "id" => "1", "q" => "z" }})),
  JSON.parse(Views::Widgets.plain_json(widget, true, {{ "id" => "1" }})),
])
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
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        r#"[{"name":"b","owner":"x","q":"z"},{"name":"b","q":null}]"#
    );
}
