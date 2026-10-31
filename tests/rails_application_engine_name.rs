//! PostgreSQL: rails application engine name, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const APPLICATION: &str = r#"require_relative "boot"
require "rails/all"

module Acme
  class Application < Rails::Application
    config.load_defaults 8.0
  end
end
"#;

const ENGINE: &str = r#"module Foo
  class << self
    attr_accessor :app_name
  end

  class Engine < Rails::Engine
    initializer "foo.configure" do |app|
      Foo.app_name = app.engine_name.sub(/_application\z/, "")
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn an_initializer_reads_the_applications_engine_name() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_rails_engine_name\n")
        .write("config/application.rb", APPLICATION)
        .write("lib/foo/engine.rb", ENGINE)
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
             require_relative 'runtime/rails_configuration'\n\
             Rails.env_name = 'test'\n\
             require_relative 'config/application'\n\
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_engine'\n\
             require_relative 'runtime/active_support_deprecation'\n\
             require_relative 'app/models'\n\
             Rails::Railtie.run_initializers_of_railties(Rails.application, :all)\n\
             puts Rails.application.engine_name\n\
             puts Foo.app_name\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "acme_application\nacme\n", "{stderr}");
}
