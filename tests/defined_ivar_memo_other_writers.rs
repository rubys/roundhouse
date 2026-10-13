//! `lower::defined_ivar_memo` rewrites `defined?(@x)` to a flag only
//! when every writer of `@x` sets it. A class-level `@cache ||= {}` in
//! one method and `return unless defined?(@cache) && @cache` in another:
//! the compound assignment sets no flag, so a flag-based guard would
//! always return early and the cache would never be cleared. The
//! emitted program must behave as it does on CRuby.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const FIXTURES: &str = r#"class Fixtures
  def self.load(key, value)
    @cache ||= {}
    @cache[key] = value
  end

  def self.unload
    return [] unless defined?(@cache) && @cache
    keys = @cache.keys
    @cache = nil
    keys
  end

  def track(v)
    instance_variable_set(:@seen, v)
  end

  def seen?
    defined?(@seen) ? true : false
  end

  def forget
    remove_instance_variable(:@seen)
  end
end
"#;

#[test]
fn a_guard_whose_ivar_has_a_writer_the_flag_cannot_follow_stays_defined() {
    let script = "p Fixtures.unload\nFixtures.load(:a, 1)\np Fixtures.unload\np Fixtures.unload\n\
        f = Fixtures.new\np f.seen?\nf.track(1)\np f.seen?\nf.forget\np f.seen?\n";
    let native = emit_and_run::ruby().args(["-W:no-deprecated", "-e", &format!("{FIXTURES}\n{script}")]).output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));

    let (tree, errors) = emit_and_run::empty_app().write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n").write("config/routes.rb", "Rails.application.routes.draw do\nend\n").write("app/models/fixtures.rb", FIXTURES).emit(BuildTarget::Ruby);
    let emitted = tree.join("app/models/fixtures.rb");
    let text = std::fs::read_to_string(&emitted).unwrap_or_else(|_| panic!("no {}: {errors:#?}", emitted.display()));
    assert!(!text.contains("_defined"), "no flag for an ivar a compound/reflective writer sets:\n{text}");

    let run = std::process::Command::new("ruby")
        .current_dir(&tree)
        .env("BLOG_DB", ":memory:")
        .arg("-e")
        .arg(format!("require File.expand_path(\"main\", Dir.pwd)\nMain.configure_default_adapter!\n{script}"))
        .output()
        .expect("spawn ruby");
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));
    assert_eq!(String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&native.stdout));
}
