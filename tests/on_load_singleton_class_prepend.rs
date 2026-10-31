//! PostgreSQL: on load singleton class prepend, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ENGINE: &str = r#"require "rails"

module Foo
  class Engine < Rails::Engine
    initializer "foo.patches" do |app|
      Dir[config.root.join("lib/foo/patches/**/*.rb")].each { |file| require file }
    end
  end
end
"#;

const ACCESSOR_PATCH: &str = r#"module ClassAccessorPatch
  def current_value
    Thread.current.thread_variable_get(:class_accessor_patch)
  end

  def current_value=(value)
    Thread.current.thread_variable_set(:class_accessor_patch, value)
  end
end

ActiveSupport.on_load(:active_record) do
  singleton_class.prepend(ClassAccessorPatch)
end
"#;

#[test]
#[ignore = "fork spec: postgresql layout"]
fn singleton_class_prepend_in_a_load_hook_registers_on_the_base() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_singleton_prepend_hook\n")
        .write("lib/foo/engine.rb", ENGINE)
        .write("lib/foo/patches/class_accessor_patch.rb", ACCESSOR_PATCH)
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .emit(BuildTarget::Ruby);
    let program = std::fs::read_to_string(dir.join("app/models/roundhouse_files/lib/foo/patches/class_accessor_patch.rb"))
        .expect("the patch's file program");
    assert!(program.contains("ActiveSupport.on_load(:active_record) do"), "{program}");
    assert!(program.contains("singleton_class.prepend(ClassAccessorPatch)"), "{program}");

    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-e",
            "require_relative 'runtime/active_support_lazy_load_hooks'\n\
             $LOAD_PATH.unshift(File.expand_path('runtime/features'))\n\
             require_relative 'runtime/module_delegate'\n\
             require_relative 'runtime/active_support_inflector'\n\
             require_relative 'runtime/rails'\n\
             Rails.env_name = 'test'\n\
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_engine'\n\
             require_relative 'runtime/active_support_deprecation'\n\
             require_relative 'app/models'\n\
             Rails::Railtie.run_initializers_of_railties(Rails.application, :finish)\n\
             base = Class.new\n\
             puts base.respond_to?(:current_value=)\n\
             ActiveSupport.run_load_hooks(:active_record, base)\n\
             base.current_value = 7\n\
             puts base.current_value\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "false\n7\n", "{stderr}");
    assert!(!stderr.contains("error["), "{stderr}");
}
