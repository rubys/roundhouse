//! PostgreSQL: rails config cache classes, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

#[test]
#[ignore = "fork spec: postgresql"]
fn config_cache_classes_reads_true_as_the_tree_never_reloads() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_cache_classes\n")
        .write("config/application.rb", "require \"rails\"\n\nmodule CacheApp\n  class Application < Rails::Application\n  end\nend\n")
        .emit(BuildTarget::Ruby);
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-e",
            "require_relative 'runtime/active_support_lazy_load_hooks'\n\
             $LOAD_PATH.unshift(File.expand_path('runtime/features'))\n\
             require_relative 'runtime/module_delegate'\n\
             require_relative 'runtime/active_support_inflector'\n\
             require_relative 'runtime/rails'\n\
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_configuration'\n\
             require_relative 'config/application'\n\
             config = Rails.application.config\n\
             p config.cache_classes\n\
             p config.reloading_enabled?\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "true\nfalse\n");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn config_active_job_custom_serializers_reach_the_serializer_registry() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_custom_serializers\n")
        .write("app/jobs/application_job.rb", "class ApplicationJob < ActiveJob::Base\n  before_perform { |job| job }\nend\n")
        .write("config/application.rb", "require \"rails\"\n\nmodule SerializerApp\n  class Application < Rails::Application\n  end\nend\n")
        .emit(BuildTarget::Ruby);
    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-e",
            "require_relative 'runtime/active_support_lazy_load_hooks'\n\
             $LOAD_PATH.unshift(File.expand_path('runtime/features'))\n\
             require_relative 'runtime/module_delegate'\n\
             require_relative 'runtime/active_support_inflector'\n\
             require_relative 'runtime/rails'\n\
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_configuration'\n\
             require_relative 'runtime/active_job_callbacks'\n\
             require_relative 'config/application'\n\
             class MoneySerializer < ActiveJob::Serializers::ObjectSerializer\n\
               def klass = Money\n\
               def serialize?(argument) = argument.is_a?(Money)\n\
               def serialize(money) = super('cents' => money.cents)\n\
               def deserialize(hash) = Money.new(hash['cents'])\n\
             end\n\
             Money = Struct.new(:cents)\n\
             config = Rails.application.config\n\
             p config.active_job.custom_serializers\n\
             config.active_job.custom_serializers << MoneySerializer\n\
             p ActiveJob::Serializers.serializers.include?(MoneySerializer.instance)\n\
             ActiveSupport.run_load_hooks(:after_initialize, Rails.application)\n\
             p ActiveJob::Serializers.serializers.include?(MoneySerializer.instance)\n\
             p ActiveJob::Serializers.deserialize(ActiveJob::Serializers.serialize(Money.new(5)))\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "[]\nfalse\ntrue\n#<struct Money cents=5>\n");
}
