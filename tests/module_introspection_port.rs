//! PostgreSQL: module introspection port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

const BRANDING: &str = "module Admin\n  module Configs\n    class Branding\n      def self.branded? = module_parents.include?(Admin::Configs)\n    end\n  end\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_module_introspection_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/admin/configs/branding.rb", BRANDING)
}

const SCRIPT: &str = r#"require "active_support"
require "active_support/inflector"
puts "defined before: #{Module.method_defined?(:module_parents)}"
require ARGV.fetch(0)
module Admin; module Configs; class Branding; end; end; end
anon = Class.new
p Admin::Configs::Branding.module_parents
p Admin::Configs::Branding.module_parent
p Admin::Configs::Branding.module_parent_name
p Admin.module_parents
p Admin.module_parent
p Admin.module_parent_name
p anon.module_parents
p anon.module_parent
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn module_parents_is_loaded_at_boot_where_app_code_calls_it() {
    let (tree, _) = app().emit(BuildTarget::Ruby);
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(
        facades.contains("require_relative \"active_support_module_introspection\"\n"),
        "module_parents not loaded at boot:\n{facades}"
    );
    assert!(tree.join("runtime/features/active_support/core_ext/module/introspection.rb").exists());
}

#[test]
#[ignore = "fork spec: postgresql"]
fn module_parents_answers_as_rails_does() {
    let (tree, _) = app().emit(BuildTarget::Ruby);
    let port = tree.join("runtime/active_support_module_introspection.rb");
    let out = emit_and_run::ruby().arg("-e").arg(SCRIPT).arg(&port).output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        stdout,
        "defined before: false\n\
         [Admin::Configs, Admin, Object]\n\
         Admin::Configs\n\
         \"Admin::Configs\"\n\
         [Object]\n\
         Object\n\
         nil\n\
         [Object]\n\
         Object\n"
    );
}
