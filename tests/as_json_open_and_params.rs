//! `as_json` on a value with no static type, and on a synthesized
//! `<X>Params` class.
//!
//! The `as_json` grounding (`ActiveSupport::JSON.jsonable`) fired only
//! for a receiver typed Hash or Array, or the request's params. Two
//! sites kept a dynamic call the Spinel runtime cannot answer:
//!
//! * an untyped value read out of a Hash, `payload["order"]&.as_json`
//!   ("undefined method 'as_json' for an instance of Hash");
//! * a params helper, `widget_params.as_json`, whose synthesized class
//!   had `to_h` and no `as_json` ("undefined method 'as_json' for an
//!   instance of WidgetParams").
//!
//! The first now goes through `jsonable`, which answers a Hash, an Array
//! or a scalar as Rails' core_ext does; the second class gets an
//! `as_json` (its `to_h`, as `Parameters#as_json` answers the permitted
//! values String-keyed) when a call site asks for one.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use roundhouse::ingest::ingest_app_from_tree;
use roundhouse::project::{target_files, BuildTarget};

const SCHEMA: &str = "ActiveRecord::Schema.define(version: 1) do\n  \
    create_table :widgets do |t|\n    t.string :name\n    t.string :color\n    t.json :note\n  end\nend\n";

const MODEL: &str = r#"class Widget < ApplicationRecord
  def snapshot(payload)
    payload["order"]&.as_json
  end

  def fetched(payload)
    payload.fetch("order").as_json
  end
end
"#;

const CONTROLLER: &str = r#"class WidgetsController < ApplicationController
  def update
    @widget = Widget.find(params[:id])
    @widget.update!(note: widget_params.as_json)
    head :no_content
  end

  private

  def widget_params
    params.require(:widget).permit(:name, :color)
  end
end
"#;

fn emitted(target: BuildTarget) -> Vec<(String, String)> {
    let mut tree: HashMap<PathBuf, Vec<u8>> = HashMap::new();
    tree.insert(PathBuf::from("db/schema.rb"), SCHEMA.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("app/models/application_record.rb"),
        b"class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/models/widget.rb"), MODEL.as_bytes().to_vec());
    tree.insert(
        PathBuf::from("config/routes.rb"),
        b"Rails.application.routes.draw do\n  resources :widgets, only: :update\nend\n".to_vec(),
    );
    tree.insert(PathBuf::from("app/controllers/widgets_controller.rb"), CONTROLLER.as_bytes().to_vec());
    let mut app = ingest_app_from_tree(tree).expect("ingest");
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

/// The body of `def <name>` … its `end`, as emitted.
fn method<'a>(src: &'a str, name: &str) -> &'a str {
    let start = src.find(&format!("  def {name}")).unwrap_or_else(|| panic!("no def {name}:\n{src}"));
    let end = src[start..].find("\n  end\n").map(|i| start + i).unwrap();
    &src[start..end]
}

#[test]
fn an_untyped_value_read_out_of_a_hash_is_grounded() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = emitted(target);
        let src = file(&files, "models/widget.rb");
        for name in ["snapshot", "fetched"] {
            let body = method(src, name);
            assert!(body.contains("ActiveSupport::JSON.jsonable("), "{target:?} {name}:\n{body}");
            assert!(!body.contains(".as_json"), "{target:?} {name}:\n{body}");
        }
    }
}

#[test]
fn a_params_class_asked_for_as_json_answers_it() {
    for target in [BuildTarget::Ruby, BuildTarget::Spinel] {
        let files = emitted(target);
        let controller = file(&files, "widgets_controller.rb");
        assert!(controller.contains("self.widget_params.as_json"), "{target:?}:\n{controller}");
        let params = file(&files, "widget_params.rb");
        let body = method(params, "as_json");
        assert!(body.contains("to_h"), "{target:?}:\n{params}");
    }
}

/// The emitted `snapshot`, run on CRuby with the runtime's
/// `ActiveSupport::JSON` (no core extensions), answers Rails' values.
#[test]
fn the_grounded_call_answers_what_rails_answers() {
    let files = emitted(BuildTarget::Ruby);
    let body = method(file(&files, "models/widget.rb"), "snapshot").to_string();
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/ruby/active_support_ext.rb");
    let script = format!(
        "require {runtime:?}\nclass Probe\n{body}\n  end\nend\n\
         w = Probe.new\n\
         p [w.snapshot({{}}), w.snapshot({{\"order\" => {{status: :open, n: 1}}}}), w.snapshot({{\"order\" => [:a]}})]\n",
        runtime = runtime.display().to_string()
    );
    let output = Command::new("ruby").args(["-e", &script]).output().unwrap();
    assert!(
        output.status.success(),
        "{script}\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        r#"[nil, {"status" => "open", "n" => 1}, ["a"]]"#
    );
}
