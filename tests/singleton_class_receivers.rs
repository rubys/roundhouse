//! `x.singleton_class`, `Module.new` and self in an instance method of
//! a `Module` subclass are proven class object receivers.
//!
//! A helper that saves and restores `Kernel.rand` does so with
//! `Kernel.singleton_class.instance_method(:rand)` and
//! `Kernel.singleton_class.define_method(:rand, saved)`. A singleton class
//! is unique to its receiver, so (unlike `Probe.new.class`, which
//! critic_admission keeps refused) no subclass can stand in for it.
//! Native Ruby on the same source is the oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const PATCH: &str = r##"module Patch
  class Box
    def label = "box"
  end

  class Recorder < Module
    def initialize(name)
      define_method(name) { :recorded }
      self.define_method(:"#{name}?") { true }
    end
  end

  def self.report
    saved = Patch.singleton_class.instance_method(:report)
    box = Box.new
    box.singleton_class.define_method(:label) { "patched" }
    Patch.singleton_class.define_method(:extra) { :extra }
    names = Patch.singleton_class.instance_methods(false).sort
    Patch.singleton_class.send(:remove_method, :extra)
    fresh = Module.new
    [:a, :b].each { |n| fresh.define_method(n) { n } }
    holder_class = Class.new
    holder_class.include(fresh)
    holder_class.include(Recorder.new(:rec))
    holder = holder_class.new
    [box.label, Box.new.label, names, saved.name, Patch.respond_to?(:extra),
     holder.public_send(:a), holder.public_send(:b), holder.public_send(:rec), holder.public_send(:rec?)]
  end
end
"##;

const REPORT: &str = "p Patch.report\n";

#[test]
fn singleton_class_receivers_run_as_in_native_ruby() {
    let dir = std::env::temp_dir().join(format!("roundhouse-singleton-receivers-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("patch.rb");
    std::fs::write(&file, PATCH).unwrap();
    let native = std::process::Command::new("ruby")
        .arg("-e")
        .arg(format!("require {:?}\n{REPORT}", file.display().to_string()))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(expected, "[\"patched\", \"box\", [:extra, :report], :report, false, :a, :b, :recorded, true]\n", "native control");

    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("lib/patch.rb", PATCH)
        .emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{}", errors.join("\n"));
    let output = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require \"./app/models/patch\"\nrequire \"./app/models/patch/box\"\nrequire \"./app/models/patch/recorder\"\n{REPORT}"))
        .current_dir(&emitted)
        .output()
        .expect("spawn ruby");
    assert!(output.status.success(), "emitted program failed in {}:\n{}", emitted.display(), String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

/// An immediate has no singleton class (`1.singleton_class` raises
/// TypeError), and an unknown receiver proves nothing.
#[test]
fn an_immediate_or_unknown_receiver_stays_refused() {
    for (n, body) in ["1.singleton_class.define_method(:x) { 1 }", "ghost.singleton_class.define_method(:x) { 1 }"].iter().enumerate() {
        let root = std::env::temp_dir().join(format!("roundhouse-singleton-refused-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, text) in [
            ("db/schema.rb", "ActiveRecord::Schema.define do\nend\n".to_string()),
            ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n".to_string()),
            ("app/lib/probe_patch.rb", format!("module ProbePatch\n  def self.go(ghost)\n    {body}\n  end\nend\n")),
        ] {
            let file = root.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_roundhouse")).arg("check").arg(&root).output().unwrap();
        let _ = std::fs::remove_dir_all(&root);
        let text = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(text.contains("define_method requires a proven class/module object receiver"), "{body}: {text}");
    }
}
