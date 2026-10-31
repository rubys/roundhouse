//! PostgreSQL: module anonymous port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

const SOURCE_PATH: &str = "module Schemas\n  def self.error_source_path(error_type)\n    resolved_type = error_type\n    while resolved_type.is_a?(Class) && resolved_type.anonymous?\n      resolved_type = resolved_type.superclass\n    end\n    resolved_type\n  end\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_module_anonymous_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/schemas.rb", SOURCE_PATH)
}

const SCRIPT: &str = r#"require "active_support"
require "active_support/inflector"
puts "defined before: #{Module.method_defined?(:anonymous?)}"
require ARGV.fetch(0)
class Named; end
module Mixin; end
anon = Class.new(Named)
nested = Class.new(anon)
p Named.anonymous?
p Mixin.anonymous?
p anon.anonymous?
p Module.new.anonymous?
p nested.superclass.superclass
named_later = Class.new
Object.const_set(:NamedLater, named_later)
p named_later.anonymous?
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn anonymous_is_loaded_at_boot_where_app_code_calls_it() {
    let (tree, _) = app().emit(BuildTarget::Ruby);
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(
        facades.contains("require_relative \"active_support_module_anonymous\"\n"),
        "anonymous? not loaded at boot:\n{facades}"
    );
    assert!(tree.join("runtime/features/active_support/core_ext/module/anonymous.rb").exists());
}

#[test]
#[ignore = "fork spec: postgresql"]
fn anonymous_answers_as_rails_does() {
    let (tree, _) = app().emit(BuildTarget::Ruby);
    let port = tree.join("runtime/active_support_module_anonymous.rb");
    let out = emit_and_run::ruby().arg("-e").arg(SCRIPT).arg(&port).output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(stdout, "defined before: false\nfalse\nfalse\ntrue\ntrue\nNamed\nfalse\n");
}
