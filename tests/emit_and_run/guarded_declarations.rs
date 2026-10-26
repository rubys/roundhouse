//! A declaration behind its own reload guard, and the `Method` class as a
//! value. discourse's `Discourse::VERSION` is written
//! `unless defined?(::Discourse::VERSION); module VERSION … end; end`, which
//! the class finder did not descend into, so every `Discourse::VERSION::STRING`
//! read was an unknown constant. aasm's invokers ask
//! `Method.method_defined?(:source_location)`.

use super::emit_and_run;

const VERSION: &str = r#"module Shelf
  unless defined?(::Shelf::VERSION)
    module VERSION
      STRING = "2026.10.0-latest"

      PARTS = STRING.split(".")
      private_constant :PARTS

      MAJOR = PARTS[0].to_i
      MINOR = PARTS[1].to_i
    end
  end
end
"#;

const PROBE: &str = r#"class ShelfProbe
  def self.version
    [Shelf::VERSION::STRING, Shelf::VERSION::MAJOR, Shelf::VERSION::MINOR]
  end

  def self.locates?
    Method.method_defined?(:source_location)
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::real_blog().write("app/models/shelf.rb", VERSION).write("app/models/shelf_probe.rb", PROBE)
}

const ASSERTIONS: &str = r##"got = [ShelfProbe.version, ShelfProbe.locates?]
raise got.inspect unless got == [["2026.10.0-latest", 2026, 10], true]
puts "guarded declarations passed"
"##;

#[test]
fn guarded_declarations_run() {
    let run = app().run_ruby(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("guarded declarations passed"), "{}", run.stdout);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn guarded_declarations_run_on_spinel() {
    let run = app().run_spinel(ASSERTIONS);
    run.assert_passes();
    assert!(run.stdout.contains("guarded declarations passed"), "{}", run.stdout);
}

/// Spinel answers `Method` as a value but not a class test against it:
/// `case x when Method` compiled and took the wrong arm.
#[test]
fn a_class_test_against_method_is_unsupported_on_spinel() {
    let (_emitted, errors) = emit_and_run::real_blog()
        .write("app/models/kind_probe.rb", "class KindProbe\n  def self.kind(x)\n    x.is_a?(Method)\n  end\n\n  def self.which(x)\n    case x\n    when String, Method then 1\n    else 2\n    end\n  end\nend\n")
        .emit(roundhouse::project::BuildTarget::Spinel);
    assert_eq!(errors.iter().filter(|e| e.contains("a class test against `Method`")).count(), 2, "{errors:#?}");
}
