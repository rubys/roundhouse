//! `self.mode ||= :live` in a class or module body, through the class's
//! own class-side accessor, runs as the body loads: Ruby reads `mode` and
//! writes it only when unset. The CRuby and Spinel trees replay it where
//! the source has it, as they replay `self.mode = :live`; every other
//! target refuses it by name rather than dropping it.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

use roundhouse::project::BuildTarget;

const GEM: &str = "module Paygate\n  class << self\n    attr_accessor :mode, :region\n  end\n\n  self.region = :eu\n  self.region ||= :us\n  self.mode ||= :live\nend\n";

const SCRIPT: &str = "puts Paygate.mode.to_s\nputs Paygate.region.to_s\n";

const EXPECTED: &str = "live\neu\n";

fn overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", "PATH\n  remote: vendor/gems/paygate-1.0.0\n  specs:\n    paygate (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  paygate!\n")
        .write("config/application.rb", "require \"paygate\"\n")
        .write("vendor/gems/paygate-1.0.0/lib/paygate.rb", GEM)
}

#[test]
fn native_control() {
    let out = Command::new("ruby").args(["-e", &format!("{GEM}{SCRIPT}")]).output().expect("ruby is on PATH");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), EXPECTED);
}

#[test]
fn a_class_side_or_assign_runs_where_it_stands_on_ruby() {
    let run = overlay().run_ruby(SCRIPT);
    assert!(run.errors.is_empty(), "{:#?}", run.errors);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, EXPECTED, "{}", run.stderr);
}

#[test]
#[ignore = "requires the Spinel toolchain"]
fn a_class_side_or_assign_runs_where_it_stands_on_spinel() {
    let run = overlay().run_spinel(SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, EXPECTED, "{}", run.stderr);
}

#[test]
fn a_target_without_class_body_code_refuses_the_or_assign_by_name() {
    for target in [BuildTarget::Typescript, BuildTarget::Go, BuildTarget::Rust] {
        let (_tree, errors) = overlay().emit(target);
        assert!(
            errors.iter().any(|e| e.contains("class body not supported") && e.contains("self.mode")),
            "{target:?}: {errors:#?}"
        );
    }
}

/// Through an accessor an app ancestor defines, a subclass body's write
/// is a body statement only the CRuby and JRuby trees write back; a
/// target with no class body refuses it by name rather than dropping it.
#[test]
fn a_target_without_class_body_code_refuses_an_inherited_class_side_write_by_name() {
    let gem = "module Paygate\n  class Base\n    class << self\n      attr_accessor :mode\n    end\n  end\n\n  class Child < Base\n    self.mode = :test\n  end\n\n  class Other < Base\n    self.mode ||= :live\n  end\nend\n";
    let script = "puts Paygate::Child.mode.to_s\nputs Paygate::Other.mode.to_s\n";
    let out = Command::new("ruby").args(["-e", &format!("{gem}{script}")]).output().expect("ruby is on PATH");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "test\nlive\n", "native control");
    let overlay = || {
        emit_and_run::empty_app()
            .write("roundhouse.yml", "")
            .write("Gemfile.lock", "PATH\n  remote: vendor/gems/paygate-1.0.0\n  specs:\n    paygate (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  paygate!\n")
            .write("config/application.rb", "require \"paygate\"\n")
            .write("vendor/gems/paygate-1.0.0/lib/paygate.rb", gem)
    };
    let run = overlay().run_ruby(script);
    assert!(run.success, "{}\n{}", run.stdout, run.stderr);
    assert_eq!(run.stdout, "test\nlive\n", "{}", run.stderr);
    let (_tree, errors) = overlay().emit(BuildTarget::Typescript);
    for written in ["self.mode = :test", "self.mode ||"] {
        assert!(
            errors.iter().any(|e| e.contains("class body statement") && e.contains(written)),
            "{written}: {errors:#?}"
        );
    }
}
