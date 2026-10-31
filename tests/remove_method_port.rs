//! PostgreSQL: remove method port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\n\
DEPENDENCIES\n  i18n\n";

/// The railtie's calls, in a method body (a class-body call of an app
/// class is not carried, so the emitted text names it only there).
const REOPEN: &str = "module Frameworks\n  class Application\n    def self.reset_assets\n      remove_possible_method :assets\n      remove_possible_method :assets=\n    end\n\n    def assets\n      @assets\n    end\n  end\nend\n";

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile", "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"i18n\"\n")
        .write("Gemfile.lock", LOCK)
        .write("config.ru", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_remove_method_unused\n")
        .write("db/schema.rb", "ActiveRecord::Schema.define do\n  create_table \"probes\" do |t|\n    t.string \"title\"\n  end\nend\n")
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/frameworks/application.rb", REOPEN)
}

const SCRIPT: &str = r#"puts "defined before: #{Module.method_defined?(:remove_possible_method)}"
klass = Class.new do
  def kept = :kept
  def dropped = :dropped
  def self.gone = :gone
  private def secret = :secret
end
require ARGV.fetch(0)
klass.remove_possible_method(:dropped)
klass.remove_possible_method(:secret)
klass.remove_possible_method(:never_defined)
klass.remove_possible_singleton_method(:gone)
klass.remove_possible_singleton_method(:never_defined)
obj = klass.new
p [obj.respond_to?(:kept), obj.respond_to?(:dropped), obj.respond_to?(:secret, true), klass.respond_to?(:gone)]
p klass.remove_possible_method(:dropped)
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn remove_possible_method_is_loaded_at_boot_where_app_code_calls_it() {
    let (tree, _) = app().emit(BuildTarget::Ruby);
    let facades = std::fs::read_to_string(tree.join("runtime/gem_facades.rb")).unwrap();
    assert!(
        facades.contains("require_relative \"active_support_remove_method\"\n"),
        "remove_possible_method not loaded at boot:\n{facades}"
    );
    assert!(tree.join("runtime/features/active_support/core_ext/module/remove_method.rb").exists());
}

#[test]
#[ignore = "fork spec: postgresql"]
fn remove_possible_method_answers_as_rails_does() {
    let (tree, _) = app().emit(BuildTarget::Ruby);
    let port = tree.join("runtime/active_support_remove_method.rb");
    let out = emit_and_run::ruby().arg("-e").arg(SCRIPT).arg(&port).output().expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "stdout:\n{stdout}\nstderr:\n{}", String::from_utf8_lossy(&out.stderr));
    assert_eq!(
        stdout,
        "defined before: false\n\
         [true, false, false, false]\n\
         nil\n"
    );
}
