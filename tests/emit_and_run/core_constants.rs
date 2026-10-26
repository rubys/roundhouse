//! `Range.new`, `is_a?(Proc)`, `RUBY_VERSION` and `rescue LoadError`, as
//! pure-Ruby gems write them (flag_shih_tzu, rails-settings-cached, aasm,
//! counter_culture). Each used to stop `check` with "constant not
//! supported (all targets)".

use super::emit_and_run;

const PROBE: &str = r#"class CoreProbe
  def self.resolved(value)
    value.is_a?(Proc) ? value.call : value
  end

  def self.spans(n)
    [Range.new(0, n).to_a, Range.new(0, n, true).to_a, Range.new(0, n, false).size]
  end

  def self.modern?
    RUBY_VERSION >= "2.7.1"
  end

  def self.optional
    raise LoadError, "missing"
  rescue LoadError => e
    e.message
  end

  def self.object_version
    Object::RUBY_VERSION == RUBY_VERSION
  end
end

module Inventory
  Range = Struct.new(:start, :finish)

  def self.span
    Range.new(1, 2).finish
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/core_probe.rb", PROBE)
}

const ASSERTIONS: &str = r##"got = [CoreProbe.resolved(-> { 7 }), CoreProbe.resolved(5), CoreProbe.spans(3), CoreProbe.modern?, CoreProbe.optional, CoreProbe.object_version, Inventory.span]
raise got.inspect unless got == [7, 5, [[0, 1, 2, 3], [0, 1, 2], 4], true, "missing", true, 2]
puts "core constants passed"
"##;

#[test]
fn core_constants_run() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("core constants passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn core_constants_run_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("core constants passed"), "{}", run.stdout);
}

/// No strict runtime defines `Proc`, `Range`, `LoadError` or
/// `RUBY_VERSION`; they are reported there.
#[test]
fn core_constants_are_unsupported_on_strict_targets() {
    use roundhouse::project::BuildTarget::*;
    for target in [Rust, Typescript, Go, Python, CSharp, Kotlin, Swift, Elixir, Crystal] {
        let (_emitted, errors) = app().emit(target);
        for name in ["Proc is not available", "Range is not available", "LoadError is not available"] {
            assert!(errors.iter().any(|e| e.contains(name)), "{target:?} {name}: {errors:#?}");
        }
        let version = errors.iter().filter(|e| e.contains("`RUBY_VERSION` is the running Ruby's")).count();
        assert_eq!(version, 3, "{target:?}: {errors:#?}");
    }
}
