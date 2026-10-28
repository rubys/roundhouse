//! A gem `autoload` target that is also plainly required loads once.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const ERR_KIT: &str = "require \"err-kit/errors\"\nrequire \"err-kit/validator\"\n";
const ERRORS: &str = r#"module ErrKit
  module Errors
    autoload(:DataError, "err-kit/errors/data_error")
  end
end
"#;
const DATA_ERROR: &str = "require \"set\"\n\nmodule ErrKit\n  module Errors\n    class DataError < StandardError\n      def label = \"data\"\n    end\n  end\nend\n";
const VALIDATOR: &str = r#"require "err-kit/errors/data_error"

module ErrKit
  class Validator
    def check
      raise ErrKit::Errors::DataError, "bad"
    rescue ErrKit::Errors::DataError => e
      e.label
    end
  end
end
"#;
const LOCK: &str = "PATH\n  remote: vendor/gems/err-kit-1.0.0\n  specs:\n    err-kit (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  err-kit!\n";

const FILES: [(&str, &str); 4] = [
    ("err-kit.rb", ERR_KIT),
    ("err-kit/errors.rb", ERRORS),
    ("err-kit/errors/data_error.rb", DATA_ERROR),
    ("err-kit/validator.rb", VALIDATOR),
];

#[test]
fn a_plain_require_of_an_autoloaded_gem_program_defines_its_constant_without_a_warning() {
    let mut app = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"err-kit\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n");
    for (path, text) in FILES {
        app = app.write(&format!("vendor/gems/err-kit-1.0.0/lib/{path}"), text);
    }
    let run = app.run_ruby_env(
        "require \"err-kit\" unless defined?(ErrKit::Validator)\nputs ErrKit::Validator.new.check\n",
        &[("RUBYOPT", "-w")],
    );
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert_eq!(run.stdout.lines().last(), Some("data"), "{}", run.stderr);
    assert!(!run.stderr.contains("to define"), "autoload warning in the emitted boot\n{}", run.stderr);
}
