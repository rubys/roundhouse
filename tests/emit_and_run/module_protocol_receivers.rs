//! Module protocol on receivers proven to be class objects without being
//! a constant: `x.class`, a `tap` block over a class, an `is_a?(Class)`
//! guard and `String#constantize`. Each used to stop `check` with
//! "requires a proven class/module object receiver" (forem's liquid tags,
//! counter_culture's `self.class.include?`, aasm's invokers, discourse's
//! `Class.new(base).tap { it.class_eval(&block) }`). A class object
//! handed to a constructor and read back from its ivar or reader is
//! one too (aasm's `AASM::Base#klass`).

use super::emit_and_run;

const PROBE: &str = r#"class ProtocolProbe
  VALID_CONTEXTS = [:post].freeze

  def contexts
    self.class.const_defined?(:VALID_CONTEXTS) ? self.class.const_get(:VALID_CONTEXTS) : []
  end

  def flagged?
    self.class.include?(Flagged)
  end

  def self.tapped
    ProtocolProbe.tap { |k| k.const_defined?(:VALID_CONTEXTS) }.name
  end

  def self.callable?(subject)
    subject.is_a?(Class) && subject.method_defined?(:contexts)
  end

  def self.named(name)
    name.to_s.constantize.const_defined?(:VALID_CONTEXTS)
  end

  def self.holder
    ClassHolder.new(self)
  end
end

class FlaggedProbe < ProtocolProbe
  include Flagged
end
"#;

const HOLDER: &str = r#"class ClassHolder
  attr_reader :klass

  def initialize(klass)
    @klass = klass
  end

  def contexts?
    klass.const_defined?(:VALID_CONTEXTS)
  end

  def answers?(name)
    @klass.method_defined?(name)
  end

  def self.for_probe
    ClassHolder.new(ProtocolProbe)
  end
end

class DefaultHolder
  def initialize(klass = ProtocolProbe)
    @klass = klass
  end

  def contexts?
    @klass.const_defined?(:VALID_CONTEXTS)
  end
end

class KeywordHolder
  def initialize(klass:)
    @klass = klass
  end

  def answers?(name)
    @klass.method_defined?(name)
  end

  def self.for_probe
    KeywordHolder.new(klass: FlaggedProbe)
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog()
        .write("app/models/concerns/flagged.rb", "module Flagged\n  def flag\n    true\n  end\nend\n")
        .write("app/models/protocol_probe.rb", PROBE)
        .write("app/models/class_holder.rb", HOLDER)
}

const ASSERTIONS: &str = r##"got = [ProtocolProbe.new.contexts, ProtocolProbe.new.flagged?, FlaggedProbe.new.flagged?, ProtocolProbe.tapped,
       ProtocolProbe.callable?(ProtocolProbe), ProtocolProbe.callable?(3), ProtocolProbe.named("ProtocolProbe"),
       ProtocolProbe.holder.contexts?, ClassHolder.for_probe.answers?(:contexts), FlaggedProbe.holder.answers?(:flag),
       ClassHolder.for_probe.answers?(:missing), DefaultHolder.new.contexts?, DefaultHolder.new(KeywordHolder).contexts?,
       KeywordHolder.for_probe.answers?(:flag)]
raise got.inspect unless got == [[:post], false, true, "ProtocolProbe", true, false, true, true, true, true, false, true, false, true]
puts "module protocol passed"
"##;

#[test]
fn module_protocol_on_proven_receivers_runs() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("module protocol passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn module_protocol_on_proven_receivers_runs_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("module protocol passed"), "{}", run.stdout);
}

/// A strict emit has no class object to ask; each query is reported there.
#[test]
fn module_protocol_queries_are_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget::*;
    for target in [Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Elixir, Crystal] {
        let (_emitted, errors) = app().emit(target);
        for name in ["`const_defined?` asks a class object", "`const_get` asks a class object", "`include?` asks a class object", "`method_defined?` asks a class object"] {
            assert!(errors.iter().any(|e| e.contains(name)), "{target:?} {name}: {errors:#?}");
        }
        // An app object's own method of the same name is not the Module query.
        let (_emitted, errors) = emit_and_run::real_blog()
            .write("app/models/registry.rb", "class Registry\n  def const_get(name)\n    name.to_s\n  end\n\n  def self.lookup\n    Registry.new.const_get(:x)\n  end\nend\n")
            .emit(target);
        assert!(!errors.iter().any(|e| e.contains("asks a class object")), "{target:?}: {errors:#?}");
    }
}
