//! A `require` in a gem's module body runs before the call that needs it.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ACT_KIT: &str = r#"require "act-kit/version"

module ActKit
  require "act-kit/format"

  Format.register_lazy(:json, "act-kit/formats/json")
  Format.register_lazy(:avro, "act-kit/formats/avro")

  def self.formats
    Format.registry.keys
  end
end
"#;
const VERSION: &str = "module ActKit\n  VERSION = \"1.0.0\"\nend\n";
const FORMAT: &str = r#"module ActKit
  class Format
    def self.registry
      @registry ||= {}
    end

    def self.register_lazy(name, path)
      registry[name] = path
    end
  end
end
"#;
const LOCK: &str = "PATH\n  remote: vendor/gems/act-kit-1.0.0\n  specs:\n    act-kit (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  act-kit!\n";

const FILES: [(&str, &str); 3] =
    [("act-kit.rb", ACT_KIT), ("act-kit/version.rb", VERSION), ("act-kit/format.rb", FORMAT)];

#[test]
#[ignore = "fork spec: load order"]
fn a_gem_module_body_require_runs_before_the_call_that_needs_it() {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"act-kit\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n");
    for (path, text) in FILES {
        app = app.write(&format!("vendor/gems/act-kit-1.0.0/lib/{path}"), text);
    }
    let run = app.run_ruby("puts ActKit.formats.inspect\n");
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert_eq!(run.stdout.lines().last(), Some("[:json, :avro]"), "{}", run.stderr);
}
