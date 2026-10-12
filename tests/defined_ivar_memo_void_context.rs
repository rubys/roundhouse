//! The presence flag `lower::defined_ivar_memo` sets beside each
//! `@x = …` ends the rewrite with a read of `@x` only where the
//! assignment's value is used. An `initialize` that assigns `@possible`
//! mid-branch must not emit a trailing bare `@possible`: a variable in
//! void context, which CRuby warns about at parse time ("possibly
//! useless use of a variable in void context") and a warnings-as-errors
//! boot raises on.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const CATALOG: &str = r#"class Catalog
  def initialize(types)
    if types
      @possible = types.to_h
      key, _ = @possible.first
      if key.is_a?(Symbol)
        @possible = @possible.transform_keys(&:to_s)
      end
    end
  end

  def possible
    return @possible if defined?(@possible)
    @possible = {}
  end

  def first_key
    possible.keys.first
  end
end
"#;

#[test]
fn a_memoised_assignment_whose_value_is_unused_leaves_no_void_read() {
    let script = "puts Catalog.new({ a: 1 }).first_key.inspect\nputs Catalog.new(nil).possible.inspect\n";
    let native = emit_and_run::ruby().args(["-W:no-deprecated", "-e", &format!("{CATALOG}\n{script}")]).output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));

    let (tree, errors) = emit_and_run::empty_app().write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n").write("config/routes.rb", "Rails.application.routes.draw do\nend\n").write("app/models/catalog.rb", CATALOG).emit(BuildTarget::Ruby);
    let emitted = tree.join("app/models/catalog.rb");
    let text = std::fs::read_to_string(&emitted).unwrap_or_else(|_| panic!("no {}: {errors:#?}", emitted.display()));
    assert!(text.contains("@possible_defined = true"), "the guard is lowered to a flag:\n{text}");
    let check = emit_and_run::ruby().arg("-wc").arg(&emitted).output().expect("ruby -wc");
    let stderr = String::from_utf8_lossy(&check.stderr);
    assert!(check.status.success() && !stderr.contains("void context"), "{stderr}\n{text}");

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
