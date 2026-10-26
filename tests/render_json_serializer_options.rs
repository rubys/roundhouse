//! `render json: x, serializer: S` (active_model_serializers) and its
//! siblings `each_serializer:`, `root:` and `adapter:`.
//!
//! The `render json:` lowering passed every option it did not know
//! through to the runtime's `render(body, status:, content_type:,
//! location:)`, so these became keywords that call does not take: the
//! lowered action raised `ArgumentError (unknown keywords)` on its first
//! request, and nothing reported it. Had it not raised, the body would
//! have been the value's own `to_json`, not the serializer's output.
//!
//! Each such render is now a reported residue, an error that keeps the
//! route with Rails, and the keywords are not emitted. Rendering through
//! the serializer is not modelled.
//!
//! With that in place, `serialization_scope nil` (ActionController::
//! Serialization's "pass serializers no scope") has nothing left to
//! drop: the serializer is never reached from a lowered action. It is
//! consumed instead of being the "class-body macro not recognized" gap.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("roundhouse-ams-render-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

const CONTROLLER: &str = r#"class InfoController < ActionController::Base
  serialization_scope nil

  def show
    render json: Info.new, serializer: InfoSerializer, root: "instance"
  end

  def index
    render json: [Info.new], each_serializer: InfoSerializer
  end

  def flat
    render json: Info.new, adapter: :attributes
  end

  def plain
    render json: { name: "x" }
  end
end
"#;

fn app(src: &Path) {
    write(src, "Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"active_model_serializers\"\n");
    // The lock names the gem, so `InfoSerializer < ActiveModel::Serializer`
    // resolves (a gem DSL class) and the render residue alone is what
    // keeps a serializer route with Rails.
    write(
        src,
        "Gemfile.lock",
        "GEM\n  remote: https://rubygems.org/\n  specs:\n    active_model_serializers (0.10.14)\n\nPLATFORMS\n  ruby\n\nDEPENDENCIES\n  active_model_serializers\n",
    );
    write(
        src,
        "config/routes.rb",
        "Rails.application.routes.draw do\n  get \"/info\" => \"info#show\"\n  get \"/infos\" => \"info#index\"\n  get \"/flat\" => \"info#flat\"\n  get \"/plain\" => \"info#plain\"\nend\n",
    );
    write(src, "db/schema.rb", "ActiveRecord::Schema.define do\nend\n");
    write(src, "app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  primary_abstract_class\nend\n");
    write(src, "app/models/info.rb", "class Info\n  def name = \"x\"\nend\n");
    write(src, "app/serializers/info_serializer.rb", "class InfoSerializer < ActiveModel::Serializer\n  attributes :name\nend\n");
    write(src, "app/controllers/info_controller.rb", CONTROLLER);
}

fn roundhouse(args: &[&str], out: &Path, src: &Path) -> (bool, String) {
    let run = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(args)
        .arg("-o")
        .arg(out)
        .arg(src)
        .env("ROUNDHOUSE_FUTAMURA_LEDGER", "1")
        .output()
        .unwrap();
    (run.status.success(), String::from_utf8_lossy(&run.stderr).into_owned())
}

/// The three serializer renders stay with Rails, each on the render
/// residue; the plain `render json:` beside them still lowers, and
/// `serialization_scope nil` is no gap.
#[test]
fn a_serializer_render_keeps_its_route_with_rails() {
    let src = scratch("plan-src");
    let out = scratch("plan-out");
    app(&src);
    let (ok, stderr) = roundhouse(&["--target", "futamura", "--survey"], &out, &src);
    assert!(ok, "{stderr}");
    assert!(
        stderr.contains("whole-route: 1 GET action(s) served by the lowered app; 3 left to Rails"),
        "{stderr}"
    );
    for option in ["serializer:", "each_serializer:", "adapter:"] {
        assert!(
            stderr.contains(&format!("`render json:` with `{option}` is not lowered")),
            "{option} is reported: {stderr}"
        );
    }
    assert!(!stderr.contains("`serialization_scope`"), "serialization_scope nil is consumed: {stderr}");
    assert!(!stderr.contains("constant not supported"), "only the render residue blocks: {stderr}");
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}

/// Written out anyway (`--allow-unsupported`), no lowered body passes
/// the runtime `render` a keyword it does not take.
#[test]
fn no_lowered_render_carries_a_serializer_keyword() {
    let src = scratch("emit-src");
    let out = scratch("emit-out");
    app(&src);
    let (_, stderr) = roundhouse(&["--target", "ruby", "--allow-unsupported"], &out, &src);
    let controller = fs::read_to_string(out.join("app/controllers/info_controller.rb"))
        .unwrap_or_else(|e| panic!("{e}: {stderr}"));
    for keyword in ["serializer:", "each_serializer:", "root:", "adapter:"] {
        assert!(!controller.contains(keyword), "{keyword} in:\n{controller}");
    }
    assert!(controller.contains("def plain"), "{controller}");
    fs::remove_dir_all(&src).unwrap();
    fs::remove_dir_all(&out).unwrap();
}
