//! PostgreSQL: active support core ext feature, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;
use std::path::Path;

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

// A gem's shape: the umbrella require, then a class whose superclass is
// Active Support's (`JSON::JWK < ActiveSupport::HashWithIndifferentAccess`).
const PROBE_GEM: &str = r#"require "active_support"
require "active_support/core_ext"
require "active_support/security_utils"

module ProbeGem
  class Key < ActiveSupport::HashWithIndifferentAccess
  end

  def self.report
    key = Key.new(kid: "k1")
    {
      key: [key["kid"], Key.superclass.name],
      core_ext: ["a_b".camelize, { a: nil }.compact, nil.blank?],
      secure_compare: [ActiveSupport::SecurityUtils.secure_compare("ab", "ab"), ActiveSupport::SecurityUtils.secure_compare("ab", "ac")],
    }
  end
end
"#;

fn probe() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_active_support_core_ext_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/probe.rb", "class Probe < ApplicationRecord\nend\n")
}

fn run(mode: &str, dir: &Path, gem: &Path) -> String {
    let out = emit_and_run::ruby()
        .arg(root().join("tests/framework_feature_requires.rb"))
        .arg("run")
        .arg(mode)
        .arg(dir)
        .arg(gem)
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{mode} failed\n=== stdout ===\n{stdout}\n=== stderr ===\n{}", String::from_utf8_lossy(&out.stderr));
    stdout
}

#[test]
#[ignore = "fork spec: postgresql layout"]
fn a_gems_active_support_core_ext_loads_the_ports_as_natively_rails_does() {
    let (tree, _) = probe().emit(BuildTarget::Ruby);
    let scratch = tree.parent().expect("scratch").to_path_buf();
    let lib = scratch.join("probe_gem").join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    std::fs::write(lib.join("probe_gem.rb"), PROBE_GEM).unwrap();
    let native = run("native", &scratch.join("app"), &lib);
    let emitted = run("emitted", &tree, &lib);
    assert_eq!(emitted, native, "the tree's active_support/core_ext answered the gem differently from Rails");
    assert!(native.contains("key: [\"k1\", \"ActiveSupport::HashWithIndifferentAccess\"]"), "{native}");
    assert!(native.contains("core_ext: [\"AB\", {}, true]"), "{native}");
    assert!(native.contains("secure_compare: [true, false]"), "{native}");
}
