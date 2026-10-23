//! A class-side call nothing defines is an error, not a silent unknown.
//!
//! `Klass.x`, `self.x` in a class method and `self.class.x` name the class
//! object. In a library class whose class side the registry holds whole,
//! a name no table, mixin, subclass or class-object method answers is a
//! NoMethodError in Ruby and in the emitted program, so `check` reports
//! it as it reports a miss in a model. The refusal is pinned twice: the
//! error, and the emitted program failing the same way CRuby 4.0.5 does.

use super::emit_and_run;

const PLAIN_BOX: &str = r#"class PlainBox
  def self.by_constant
    PlainBox.missing_setting
  end

  def by_class
    self.class.missing_label
  end

  def self.by_self
    self.missing_size
  end
end
"#;

const GHOSTLY: &str = "module Ghostly\n  def ghost_owner\n    \"owner\"\n  end\nend\n";

fn errors_of(overlay: emit_and_run::Overlay) -> Vec<String> {
    overlay.emit(roundhouse::project::BuildTarget::Ruby).1
}

#[test]
fn a_class_side_call_nothing_defines_is_an_error() {
    let errors = errors_of(emit_and_run::real_blog().write("app/models/plain_box.rb", PLAIN_BOX));
    for name in ["missing_setting", "missing_label", "missing_size"] {
        let want = format!("no known method `{name}` on PlainBox");
        assert!(errors.iter().any(|e| e.contains(&want)), "{want}:\n{errors:#?}");
    }
}

/// The errors are right: the emitted program raises where CRuby raises.
#[test]
fn the_emitted_program_raises_where_the_error_points() {
    let run = emit_and_run::real_blog().write("app/models/plain_box.rb", PLAIN_BOX).run_ruby(
        r#"[-> { PlainBox.by_constant }, -> { PlainBox.new.by_class }, -> { PlainBox.by_self }].each do |call|
  call.call
  puts "answered"
rescue NoMethodError => e
  puts e.message
end
"#,
    );
    assert!(!run.errors.is_empty(), "the class-side misses are reported");
    assert_eq!(
        run.stdout,
        "undefined method 'missing_setting' for class PlainBox\n\
         undefined method 'missing_label' for class PlainBox\n\
         undefined method 'missing_size' for class PlainBox\n",
        "stderr:\n{}",
        run.stderr
    );
}

/// Not refused: a hook a subclass defines (`self` may be the subclass),
/// a class-object method (`name`), a module's own singleton method, and a
/// library class's `extend`ed module, which its emission keeps.
#[test]
fn what_a_subclass_mixin_or_class_object_answers_stays_clean() {
    let run = emit_and_run::real_blog()
        .write(
            "app/models/hooked_base.rb",
            "class HookedBase\n  def self.build\n    \"#{self.prefix}-#{name}\"\n  end\n\n  def label\n    self.class.prefix\n  end\nend\n",
        )
        .write("app/models/hooked_child.rb", "class HookedChild < HookedBase\n  def self.prefix\n    \"child\"\n  end\nend\n")
        .write("app/models/ghostly.rb", GHOSTLY)
        .write(
            "app/models/extended_box.rb",
            "class ExtendedBox\n  extend Ghostly\n\n  def self.who\n    ExtendedBox.ghost_owner\n  end\nend\n",
        )
        .write("app/models/module_box.rb", "module ModuleBox\n  def self.real\n    \"r\"\n  end\n\n  def self.call_real\n    ModuleBox.real\n  end\nend\n")
        .run_ruby(
            "puts [HookedChild.build, HookedChild.new.label, ExtendedBox.who, ModuleBox.call_real].join(\",\")\n",
        );
    run.assert_passes();
    assert_eq!(run.stdout, "child-HookedChild,child,owner,r\n", "stderr:\n{}", run.stderr);
}
