//! Forms a strict Spinel build (no `--allow-unsupported`) accepts because
//! Spinel runs them as CRuby does.
//!
//! - `defined?(A::B)` of a constant path that `lower::defined_const_fold`
//!   cannot decide (`defined?(Rails::Server)`, an optional library's
//!   constant), in a method body: Spinel answers it from the whole
//!   compiled program, which has loaded by then. While the program loads
//!   (a constant initializer), CRuby answers nil for a constant whose
//!   file has not run yet and Spinel does not, so that stays refused, as
//!   does `defined?` of anything but a constant path.
//! - `File.binwrite`: no known method before.

use std::path::PathBuf;
use std::process::Command;

fn spinel_build(tag: &str, files: &[(&str, &str)]) -> (bool, String) {
    let dir: PathBuf = std::env::temp_dir().join(format!("rh-strict-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let base: &[(&str, &str)] = &[
        ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n"),
        (
            "config/routes.rb",
            "Rails.application.routes.draw do\n  get \"probe\" => \"probe#show\"\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("app/views/probe/show.html.erb", "<% if @error %><p><%= @error %></p><% end %>\n"),
    ];
    for (path, src) in base.iter().chain(files.iter()) {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, src).unwrap();
    }
    let out = Command::new(env!("CARGO_BIN_EXE_roundhouse"))
        .args(["--target", "spinel", "-o"])
        .arg(dir.join("out"))
        .arg(&dir)
        .output()
        .expect("spawn roundhouse");
    let _ = std::fs::remove_dir_all(&dir);
    (out.status.success(), String::from_utf8_lossy(&out.stderr).into_owned())
}

fn controller(body: &str) -> String {
    format!("class ProbeController < ApplicationController\n  def show\n{body}\n  end\nend\n")
}

#[test]
fn defined_of_a_constant_path_in_a_method_builds() {
    let c = controller(
        "    @error = defined?(Rails::Server) ? \"server\" : nil\n    @error = nil unless defined?(Missing::Thing)",
    );
    let (ok, err) = spinel_build("defined", &[("app/controllers/probe_controller.rb", &c)]);
    assert!(ok, "{err}");
}

#[test]
fn defined_of_a_constant_path_while_loading_is_still_refused() {
    let c = controller("    @error = Flags::SERVER");
    let flags = "class Flags\n  SERVER = defined?(Rails::Server) ? \"server\" : \"none\"\nend\n";
    let (ok, err) = spinel_build(
        "defined-load",
        &[("app/controllers/probe_controller.rb", &c), ("app/services/flags.rb", flags)],
    );
    assert!(!ok && err.contains("runtime defined? query"), "{err}");
}

#[test]
fn defined_of_anything_else_is_still_refused() {
    let c = controller("    @error = defined?(String.new) ? \"set\" : nil");
    let (ok, err) = spinel_build("defined-call", &[("app/controllers/probe_controller.rb", &c)]);
    assert!(!ok && err.contains("runtime defined? query"), "{err}");
}

#[test]
fn file_binwrite_builds() {
    let c = controller("    @error = \"x\"\n    File.binwrite(\"tmp/probe.bin\", @error)");
    let (ok, err) = spinel_build("binwrite", &[("app/controllers/probe_controller.rb", &c)]);
    assert!(ok, "{err}");
}
