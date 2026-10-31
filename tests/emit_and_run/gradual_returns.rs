use super::gradual_returns_contract as contract;
use std::process::Command;

#[test]
fn gradual_room_and_json_values_run_after_emission() {
    let run = contract::overlay().run_ruby(contract::SCRIPT);
    run.assert_passes();
    assert_eq!(run.stdout, contract::EXPECTED, "{}", run.stderr);
}

#[test]
fn gradual_room_and_json_values_match_rails() {
    let helper =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/rails_oracle.rb");
    let available = Command::new("ruby")
        .args([
            "-r",
            helper.to_str().unwrap(),
            "-e",
            "exit(RailsOracle.available?('railties') ? 0 : 1)",
        ])
        .status()
        .expect("probe Rails oracle");
    if !available.success() {
        eprintln!("skipping Rails gradual-return control: selected railties is unavailable");
        return;
    }
    let source = format!(
        "require {:?}\nRailsOracle.activate('railties')\nrequire 'rails'\nrequire 'rails/application'\nrequire 'json'\n\
         class GradualOracle < Rails::Application; end\nRails.application = GradualOracle.new\n{}\n{}\n{}",
        helper.to_str().unwrap(),
        contract::INITIALIZER,
        contract::SOURCE,
        contract::SCRIPT
    );
    let native = Command::new("ruby")
        .args(["-e", &source])
        .output()
        .expect("run Rails oracle");
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&native.stdout), contract::EXPECTED);
    let emitted = contract::overlay().run_ruby(contract::SCRIPT);
    emitted.assert_passes();
    assert_eq!(emitted.stdout, String::from_utf8_lossy(&native.stdout));
}

#[test]
fn known_unwinds_return_the_same_string_in_source_and_emitted_ruby() {
    for body in [
        r#"JSON.parse(raw).tap { explode } rescue "ok""#,
        r#"JSON.parse(raw).tap { p = Kernel.proc { return "ok" }; alias_proc = p; alias_proc[] }"#,
        r#"JSON.parse(raw).tap { begin; Kernel.raise("x", "y"); rescue RuntimeError; nil; end } rescue "ok""#,
    ] {
        let source = format!(
            "module Probe\n def self.value(raw); {body}; end\n def self.explode; raise \"x\"; end\nend\n"
        );
        let program = format!(
            "require 'json'\n{source}\nraise 'wrong unwind' unless Probe.value('42') == 'ok'\nputs 'ok'\n"
        );
        let native = Command::new("ruby")
            .args(["-e", &program])
            .output()
            .expect("run CRuby unwind oracle");
        assert!(
            native.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&native.stderr)
        );
        let emitted = super::emit_and_run::empty_app()
            .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
            .write("lib/probe.rb", &source)
            .run_ruby("raise 'wrong unwind' unless Probe.value('42') == 'ok'\nputs 'ok'\n");
        emitted.assert_passes();
        assert_eq!(
            emitted.stdout,
            String::from_utf8_lossy(&native.stdout),
            "{body}"
        );
    }
}

#[test]
fn opaque_setters_and_uncalled_callbacks_match_cruby() {
    for body in [
        r#"JSON.parse(raw).tap { Kernel.proc { return "inner" }; "ignored" }"#,
        "self.send(:instance_variable_set, :@x, JSON.parse(raw))",
        "self.public_send('instance_variable_set', :@x, JSON.parse(raw))",
        "self.__send__(:send, :instance_variable_set, :@x, JSON.parse(raw))",
        "instance_variable_set(:@x, JSON.parse(raw))",
    ] {
        let source = format!(
            "module Probe\n def self.value(raw); forward0(raw) || :fallback; end\n\
             def self.forward0(raw); forward1(raw); end\n\
             def self.forward1(raw); {body}; end\nend\n"
        );
        let assertion = "raise 'lost Integer' unless Probe.value('42') == 42\nputs 'ok'\n";
        let original = Command::new("ruby")
            .args(["-rjson", "-e", &format!("{source}\n{assertion}")])
            .output()
            .expect("CRuby unknown-return oracle");
        assert!(
            original.status.success(),
            "{body}: {}",
            String::from_utf8_lossy(&original.stderr)
        );
        let emitted = super::emit_and_run::empty_app()
            .write("db/schema.rb", "ActiveRecord::Schema.define do\nend\n")
            .write("lib/probe.rb", &source)
            .run_ruby(assertion);
        emitted.assert_passes();
        assert_eq!(
            emitted.stdout,
            String::from_utf8_lossy(&original.stdout),
            "{body}"
        );
    }
}
