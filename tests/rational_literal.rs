//! A Rational literal (`999999.9r`, `3r`, and `1/3r`, which is `1 / 3r`)
//! lowers to its exact value, `Rational(numerator, denominator)`.
//!
//! Ingest refused `RationalNode`, so a file holding a Rational
//! constant was not emitted at all. Lowering through Float would round
//! the value, so ingest builds the Kernel `Rational` call from Prism's
//! numerator and denominator. Native Ruby on the same source is the oracle.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const RAT: &str = r#"module Ratlab
  class Limits
    MAX = 999999.9r
    WHOLE = 3r

    def self.third
      1/3r
    end

    def self.report
      [
        999999.9r, 999999.9r.class, 999999.9r * 10, 999999.9r.to_s,
        3r, 3r.denominator,
        third, third + third + third, third.class,
        -2.5r, 0.1r + 0.2r == 0.3r,
      ]
    end
  end
end
"#;

const REPORT: &str = "p Ratlab::Limits.report\n";

const CONFIG: &str = "Rails.application.configure do\n  config.eager_load = false\nend\n";

#[test]
fn rational_literals_keep_their_exact_value_like_native() {
    let dir = std::env::temp_dir().join(format!("roundhouse-rational-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let rat = dir.join("limits.rb");
    std::fs::write(&rat, RAT).unwrap();
    let native = emit_and_run::ruby()
        .arg("-e")
        .arg(format!("require {:?}\n{REPORT}", rat.display().to_string()))
        .output()
        .expect("native Ruby control");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    let expected = String::from_utf8_lossy(&native.stdout).into_owned();
    assert_eq!(
        expected,
        "[(9999999/10), Rational, (9999999/1), \"9999999/10\", (3/1), 1, (1/3), (1/1), Rational, \
         (-5/2), true]\n",
        "native control"
    );

    let run = emit_and_run::empty_app()
        .write("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n")
        .write("config/environments/development.rb", CONFIG)
        .write("lib/ratlab/limits.rb", RAT)
        .run_ruby(REPORT);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    let source = std::fs::read_to_string(run.emitted.join("app/models/ratlab/limits.rb"))
        .expect("emitted limits");
    for lowered in [
        "MAX = Rational(9999999, 10)",
        "1 / Rational(3, 1)",
    ] {
        assert!(
            source.contains(lowered),
            "missing `{lowered}` in:\n{source}"
        );
    }
    assert_eq!(run.stdout, expected);
}

/// A numerator or denominator wider than 64 bits has no integer literal to
/// become (the same limit an integer literal has), so it is refused by name
/// rather than rounded through a Float.
#[test]
fn a_rational_literal_wider_than_64_bits_is_refused_by_name() {
    let source = "module Ratlab\n  class Wide\n    BIG = 123456789012345678901234567890.5r\n  end\nend\n";
    let err = roundhouse::ingest::ingest_library_class(source.as_bytes(), "wide.rb")
        .expect_err("a wide rational is refused");
    assert!(
        format!("{err:?}").contains("rational literal's numerator or denominator does not fit in a 64-bit integer"),
        "{err:?}"
    );
}
