//! A gem's files nothing requires stay unloaded, eager loading included.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const COINKIT: &str = "module Coinkit\n  def self.value = 1\nend\n\nrequire_relative \"rubocop/cop/coinkit\" if defined?(RuboCop)\n";
const COPS: &str = "require \"rubocop/cop/coinkit/missing_currency\"\n";
const COP: &str = "module RuboCop\n  module Cop\n    module Coinkit\n      class MissingCurrency < Base\n      end\n    end\n  end\nend\n";

const SCRIPT: &str = "require \"coinkit\" unless defined?(Coinkit)\nputs Coinkit.value\nputs defined?(RuboCop).inspect\n";

const LOCK: &str = "PATH\n  remote: vendor/gems/coinkit-1.0.0\n  specs:\n    coinkit (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  coinkit!\n";

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes).lines().map(str::to_string).collect()
}

#[test]
#[ignore = "fork spec: autoload"]
fn a_namespace_only_unloaded_gem_files_open_stays_undefined() {
    unloaded_namespace(false);
}

/// Rails' `eager_load!` walks the autoload paths and Zeitwerk loaders,
/// never a plain gem's `lib/`, so eager loading loads the cops no more
/// than lazy boot does.
#[test]
#[ignore = "fork spec: autoload"]
fn eager_loading_leaves_a_plain_gems_unrequired_files_unloaded() {
    unloaded_namespace(true);
}

fn unloaded_namespace(eager: bool) {
    let root = std::env::temp_dir().join(format!("roundhouse-gem-source-unloaded-namespace-{}-{eager}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let lib = root.join("coinkit/lib");
    std::fs::create_dir_all(lib.join("rubocop/cop/coinkit")).unwrap();
    std::fs::write(lib.join("coinkit.rb"), COINKIT).unwrap();
    std::fs::write(lib.join("rubocop/cop/coinkit.rb"), COPS).unwrap();
    std::fs::write(lib.join("rubocop/cop/coinkit/missing_currency.rb"), COP).unwrap();
    let native = emit_and_run::ruby().arg("-I").arg(&lib).args(["-e", SCRIPT]).output().expect("CRuby control");
    assert!(native.status.success(), "{}", String::from_utf8_lossy(&native.stderr));
    let expected = lines(&native.stdout);
    assert_eq!(expected, ["1", "nil"], "native control");
    let _ = std::fs::remove_dir_all(&root);

    let run = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"coinkit\"\n")
        .write("config/application.rb", "Bundler.require(*Rails.groups)\n")
        .write(
            "config/environments/development.rb",
            &format!("Rails.application.configure do\n  config.eager_load = {eager}\nend\n"),
        )
        .write("vendor/gems/coinkit-1.0.0/lib/coinkit.rb", COINKIT)
        .write("vendor/gems/coinkit-1.0.0/lib/rubocop/cop/coinkit.rb", COPS)
        .write("vendor/gems/coinkit-1.0.0/lib/rubocop/cop/coinkit/missing_currency.rb", COP)
        .run_ruby(SCRIPT);
    assert!(
        run.success,
        "emitted tree failed in {}\n=== stdout ===\n{}\n=== stderr ===\n{}",
        run.emitted.display(),
        run.stdout,
        run.stderr
    );
    assert_eq!(lines(run.stdout.as_bytes()), expected, "emitted boot diverges from CRuby");
}
