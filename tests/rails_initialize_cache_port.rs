//! PostgreSQL: rails initialize cache port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ENGINE: &str = r#"module Foo
  class Engine < Rails::Engine
    initializer "foo.initialize_proxy_cache_store", before: :initialize_cache do
      Rails.configuration.cache_store = Foo::LocalStore.new
    end
  end
end
"#;

const STORE: &str = r#"module Foo
  class LocalStore < ::ActiveSupport::Cache::MemoryStore
    def with_high_throughput_local_cache(ttl: nil)
      yield
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn a_store_an_initializer_configures_before_initialize_cache_is_rails_cache() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_rails_initialize_cache\n")
        .write("lib/foo/engine.rb", ENGINE)
        .write("app/models/foo/local_store.rb", STORE)
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
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_engine'\n\
             require_relative 'runtime/active_support_deprecation'\n\
             require_relative 'runtime/active_support_cache'\n\
             require_relative 'app/models'\n\
             Rails::Railtie.run_initializers_of_railties(Rails.application, :finish)\n\
             puts Rails.cache.class\n\
             Rails.cache.write('k', 1)\n\
             puts Rails.cache.with_high_throughput_local_cache(ttl: 5) { Rails.cache.read('k') }\n\
             puts Rails.cache.equal?(Rails.configuration.cache_store)\n\
             Rails.instance_variable_set(:@cache_store, nil)\n\
             Rails.configuration.cache_store = :null_store\n\
             Rails::Railtie.send(:initialize_cache_step, Rails.application)\n\
             puts Rails.instance_variable_get(:@cache_store).inspect\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "Foo::LocalStore\n1\ntrue\nnil\n", "{stderr}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_initializer_builds_its_store_from_config_for_and_an_app_store_class() {
    const ENGINE: &str = r#"module Foo
  class Engine < Rails::Engine
    initializer "foo.initialize_proxy_cache_store", before: :initialize_cache do
      settings = Rails.application.config_for(:stores)
      Rails.configuration.cache_store = ActiveSupport::Cache.lookup_store(:app_store, settings[:servers], namespace: settings[:namespace])
    end
  end
end
"#;
    const APP_STORE: &str = r#"module ActiveSupport
  module Cache
    class AppStore < ::ActiveSupport::Cache::MemoryStore
      attr_reader :servers

      def initialize(servers, **options)
        @servers = servers
        super(**options)
      end
    end
  end
end
"#;
    const YML: &str = "base: &base\n  namespace: shared\n  timeout: 1\n\nshared:\n  <<: *base\n  nested:\n    a: 1\n\ntest:\n  :servers:\n    - \"<%= ['mc', 'test'].join('-') %>:11211\"\n  nested:\n    b: 2\n";
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_rails_initialize_cache\n")
        .write("config/stores.yml", YML)
        .write("lib/foo/engine.rb", ENGINE)
        .write("app/models/active_support/cache/app_store.rb", APP_STORE)
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
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_engine'\n\
             require_relative 'runtime/active_support_deprecation'\n\
             require_relative 'runtime/active_support_cache'\n\
             require_relative 'app/models'\n\
             c = Rails.application.config_for(:stores)\n\
             puts c.class, c.to_h.inspect, c.namespace\n\
             Rails::Railtie.run_initializers_of_railties(Rails.application, :finish)\n\
             puts Rails.cache.class, Rails.cache.servers.inspect\n\
             begin; Rails.application.config_for(:absent); rescue => e; puts e.message.sub(Dir.pwd, '.'); end\n\
             begin; ActiveSupport::Cache.lookup_store(:redis_cache_store); rescue => e; puts e.message; end\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(
        stdout,
        "ActiveSupport::OrderedOptions\n\
         {namespace: \"shared\", timeout: 1, nested: {a: 1, b: 2}, servers: [\"mc-test:11211\"]}\n\
         shared\n\
         ActiveSupport::Cache::AppStore\n\
         [\"mc-test:11211\"]\n\
         Could not load configuration. No such file - ./config/absent.yml\n\
         Could not find cache store adapter for redis_cache_store (ActiveSupport::Cache::RedisCacheStore needs the redis gem, which is not ported)\n",
        "{stderr}"
    );
}
