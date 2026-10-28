//! A gem's `Dir` glob loop of `autoload` calls registers each constant, as
//! natively.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use std::process::Command;

use roundhouse::project::BuildTarget;

const ORACLE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/support/rails_oracle.rb");

const FILES: &[(&str, &str)] = &[
    ("lib/paygate.rb", "require \"paygate/base\"\nrequire \"paygate/gateways\"\n"),
    (
        "lib/paygate/base.rb",
        "module Paygate\n  class Base\n    @@all = []\n\n    def self.inherited(subclass)\n      super\n      @@all << subclass.name\n    end\n\n    def self.all = @@all\n  end\nend\n",
    ),
    (
        "lib/paygate/gateways.rb",
        "require \"pathname\"\n\nmodule Paygate\n  load_path = Pathname.new(__FILE__ + \"/../..\")\n  Dir[File.dirname(__FILE__) + \"/gateways/**/*.rb\"].each do |filename|\n    gateway_name      = File.basename(filename, \".rb\")\n    gateway_classname = \"#{gateway_name}_gateway\".camelize\n    gateway_filename  = Pathname.new(filename).relative_path_from(load_path).sub_ext(\"\")\n\n    autoload(gateway_classname, gateway_filename)\n  end\nend\n",
    ),
    ("lib/paygate/gateways/bogus.rb", "module Paygate\n  class BogusGateway < Base\n    CODE = :ok\n  end\nend\n"),
    (
        "lib/paygate/gateways/card_net.rb",
        "require \"set\"\n\nmodule Paygate\n  class CardNetGateway < Base\n    APPROVED, DECLINED = 1, \"declined\"\n    SEEN = Set.new([APPROVED])\n\n    def self.codes = [APPROVED, DECLINED, SEEN.include?(1)]\n  end\nend\n",
    ),
];

const SCRIPT: &str = r#"p Paygate::Base.all
p Paygate::BogusGateway::CODE
p Paygate::CardNetGateway.codes
p Paygate::Base.all
p Paygate.autoload?(:BogusGateway)
"#;

const EXPECTED: &str = "[]\n:ok\n[1, \"declined\", true]\n[\"Paygate::BogusGateway\", \"Paygate::CardNetGateway\"]\nnil\n";

fn native() -> String {
    let dir = std::env::temp_dir().join(format!("rh_gem_glob_autoload_native_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in FILES {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let main = format!(
        "require {ORACLE:?}\nRailsOracle.activate(\"activesupport\")\nrequire \"active_support\"\n\
         require \"active_support/core_ext/string/inflections\"\n$LOAD_PATH.unshift({:?})\nrequire \"paygate\"\n{SCRIPT}",
        dir.join("lib").display().to_string()
    );
    std::fs::write(dir.join("main.rb"), main).unwrap();
    let out = Command::new("ruby").arg("main.rb").current_dir(&dir).output().expect("ruby is on PATH");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "native failed:\n{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_dir_all(&dir).unwrap();
    stdout
}

fn overlay() -> emit_and_run::Overlay {
    let mut overlay = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", "PATH\n  remote: vendor/gems/paygate-1.0.0\n  specs:\n    paygate (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  paygate!\n")
        .write("config/application.rb", "require \"paygate\"\n");
    for (path, text) in FILES {
        overlay = overlay.write(&format!("vendor/gems/paygate-1.0.0/{path}"), text);
    }
    overlay
}

#[test]
#[ignore = "fork spec: autoload"]
fn a_gem_glob_autoload_loop_registers_each_constant_as_native() {
    assert_eq!(native(), EXPECTED, "native control");
    let run = overlay().run_ruby(SCRIPT);
    assert!(run.success, "emitted tree failed in {}\n{}\n{}", run.emitted.display(), run.stdout, run.stderr);
    assert_eq!(run.stdout, EXPECTED, "{}", run.stderr);
    assert!(!run.stderr.contains("file_statement_not_carried"), "{}", run.stderr);
}

fn global_write_overlay() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", "PATH\n  remote: vendor/gems/payoff-1.0.0\n  specs:\n    payoff (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  paygate!\n")
        .write("config/application.rb", "require \"paygate\"\n")
        .write(
            "vendor/gems/paygate-1.0.0/lib/paygate.rb",
            "module Paygate\n  $paygate_loaded = true\n\n  def self.ready = true\nend\n",
        )
}

#[test]
#[ignore = "fork spec: autoload"]
fn a_module_body_global_write_runs_in_place() {
    // A global write in a module body is a body statement: the Ruby
    // family writes it back where it stands (it was dropped and reported
    // before `expr::BODY_STATEMENT`), so it runs as the body loads.
    let run = global_write_overlay().run_ruby("p Paygate.ready\np $paygate_loaded\n");
    assert!(run.errors.is_empty(), "{:#?}", run.errors);
    assert!(!run.stderr.contains("file_statement_not_carried"), "{}", run.stderr);
    assert_eq!(run.stdout, "true\ntrue\n", "{}", run.stderr);
}

#[test]
#[ignore = "fork spec: autoload"]
fn a_strict_target_refuses_the_module_body_global_write_by_name() {
    let (_tree, errors) = global_write_overlay().emit(BuildTarget::Spinel);
    assert!(
        errors.iter().any(|e| e.contains("class body statement") && e.contains("$paygate_loaded = true")),
        "{errors:#?}"
    );
}

const SYM_FILES: &[(&str, &str)] = &[
    ("lib/payoff.rb", "require \"payoff/integrations\"\n\nmodule Payoff\n  def self.integration(name)\n    Integrations.const_get(name)\n  end\nend\n"),
    (
        "lib/payoff/integrations.rb",
        "module Payoff\n  module Integrations\n    Dir[File.dirname(__FILE__) + '/integrations/*.rb'].each do |f|\n      # Get camelized class name\n      filename = File.basename(f, '.rb')\n\n      integration_class = filename.camelize.to_sym\n\n      autoload integration_class, f\n    end\n  end\nend\n",
    ),
    ("lib/payoff/integrations/paypal.rb", "module Payoff\n  module Integrations\n    module Paypal\n      def self.service_url = \"https://paypal.test\"\n    end\n  end\nend\n"),
    ("lib/payoff/integrations/mollie.rb", "module Payoff\n  module Integrations\n    module Mollie\n      class API\n        def self.host = \"api.mollie.test\"\n      end\n    end\n  end\nend\n"),
    ("lib/payoff/integrations/bit_pay.rb", "module Payoff\n  module Integrations\n    module BitPay\n      KIND = :crypto\n    end\n  end\nend\n"),
];

const SYM_SCRIPT: &str = r#"p Payoff::Integrations.constants.sort
p Payoff::Integrations::Paypal.service_url
p Payoff.integration("BitPay")::KIND
p Payoff::Integrations.autoload?(:Paypal)
p Payoff::Integrations::Mollie::API.host
"#;

const SYM_EXPECTED: &str = "[:BitPay, :Mollie, :Paypal]\n\"https://paypal.test\"\n:crypto\nnil\n\"api.mollie.test\"\n";

#[test]
#[ignore = "fork spec: autoload"]
fn a_gem_glob_autoload_loop_naming_symbols_registers_each_constant_as_native() {
    let dir = std::env::temp_dir().join(format!("rh_gem_glob_autoload_sym_native_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in SYM_FILES {
        let file = dir.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }
    let main = format!(
        "require {ORACLE:?}\nRailsOracle.activate(\"activesupport\")\nrequire \"active_support\"\n\
         require \"active_support/core_ext/string/inflections\"\n$LOAD_PATH.unshift({:?})\nrequire \"payoff\"\n{SYM_SCRIPT}",
        dir.join("lib").display().to_string()
    );
    std::fs::write(dir.join("main.rb"), main).unwrap();
    let out = Command::new("ruby").arg("main.rb").current_dir(&dir).output().expect("ruby is on PATH");
    assert!(out.status.success(), "native failed:\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(String::from_utf8_lossy(&out.stdout), SYM_EXPECTED, "native control");
    std::fs::remove_dir_all(&dir).unwrap();

    let mut overlay = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", "GEM\n  remote: https://rubygems.org/\n  specs:\n\nDEPENDENCIES\n  payoff!\n")
        .write("config/application.rb", "require \"payoff\"\n");
    for (path, text) in SYM_FILES {
        overlay = overlay.write(&format!("vendor/gems/payoff-1.0.0/{path}"), text);
    }
    let run = overlay.run_ruby(SYM_SCRIPT);
    assert!(run.success, "emitted tree failed in {}\n{}\n{}", run.emitted.display(), run.stdout, run.stderr);
    assert_eq!(run.stdout, SYM_EXPECTED, "{}", run.stderr);
    assert!(!run.stderr.contains("file_statement_not_carried"), "{}", run.stderr);
}
