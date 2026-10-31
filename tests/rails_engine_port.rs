//! PostgreSQL: rails engine port, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const ENGINE: &str = r#"require "rails"
require "foo/settings"

module Foo
  LIMIT = -> { 3 }

  class Engine < Rails::Engine
    if Rails.env.test?
      paths.add("test", load_path: true)
    end

    config.foo = foo_config = Settings.new(4)

    Rails.autoloaders.main.on_load("Foo::Settings") { |klass| klass }

    # First in the class: a later one gets `after:` the one before it
    # (railties initializable.rb), which `before: :set_load_path` cycles.
    initializer "foo.first", before: :set_load_path do
      ORDER << "foo.first"
    end

    initializer "foo.patches", after: :load_config_initializers do |app|
      Dir[config.root.join("lib/foo/patches/**/*.rb")].each { |file| require file }
      ORDER << "foo.patches #{foo_config.size}"
    end

    initializer "foo.second", after: "foo.patches" do
      require "foo/settings"
      ORDER << "foo.second #{LIMIT.call} #{"x".shout}"
    end

    config.to_prepare do
      ORDER << "to_prepare"
    end

    config.after_initialize do
      ORDER << "after_initialize"
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql layout"]
fn an_app_engine_is_carried_and_runs_its_body_initializers_and_hooks_in_rails_order() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_rails_engine_port\n")
        .write("lib/foo/engine.rb", ENGINE)
        .write("lib/foo/settings.rb", "module Foo\n  class Settings\n    def initialize(size)\n      @size = size\n    end\n\n    def size\n      @size\n    end\n  end\nend\n")
        .write("lib/foo/patches/string_shout.rb", "class String\n  def shout\n    upcase + \"!\"\n  end\nend\n")
        // A statement-only patch is carried as its file program; a file
        // with nothing to carry is named where the glob matches it.
        .write("lib/foo/patches/statement_only.rb", "Comparable.instance_method(:clamp)\n")
        .write("lib/foo/patches/notes.rb", "# frozen_string_literal: true\n")
        // A patch whose load raises is named, and the glob's later files
        // (string_shout.rb) still load. Not eager loaded, which would
        // require it again and raise, as Rails' eager load would.
        .write("lib/foo/patches/a_broken.rb", "module Broken\n  VALUE = Integer(\"x\")\nend\n")
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .emit(BuildTarget::Ruby);
    let engine = std::fs::read_to_string(dir.join("app/models/foo/engine.rb")).expect("the engine is carried");
    assert!(engine.contains("class Engine < Rails::Engine"), "{engine}");
    assert!(engine.contains("run_body_statement(\"lib/foo/engine.rb:8\") do"), "{engine}");
    assert!(engine.contains("require RoundhouseAutoload.path(\"models/foo/settings\")"), "{engine}");
    assert!(
        engine.contains("RoundhouseAutoload.require_carried_files(\"lib/foo/patches/**/*.rb\", [\"models/broken\", \"models/roundhouse_files/lib/foo/patches/statement_only\", \"models/string\"], [\"lib/foo/patches/notes.rb\"])"),
        "{engine}"
    );
    let models = std::fs::read_to_string(dir.join("app/models.rb")).expect("app/models.rb");
    let root = models
        .find("Rails::Engine.source_roots[\"Foo::Engine\"] = File.expand_path(\"../\", RoundhouseAutoload::ROOT)")
        .unwrap_or_else(|| panic!("{models}"));
    // engine.rb's top-level requires make it a file program, which
    // loads in the engine's place.
    let load = models
        .find("  \"models/roundhouse_files/lib/foo/engine\",\n].each { |file| require RoundhouseAutoload.path(file) }")
        .unwrap_or_else(|| panic!("{models}"));
    let program = std::fs::read_to_string(dir.join("app/models/roundhouse_files/lib/foo/engine.rb")).expect("engine.rb's program");
    assert!(program.contains("# lib/foo/engine.rb:1: `require \"rails\"` is a Rails feature this tree's runtime ports answer; not required\nrequire RoundhouseAutoload.path(\"models/foo/settings\")\nrequire RoundhouseAutoload.path(\"models/foo\")\nrequire RoundhouseAutoload.path(\"models/foo/engine\")\n"), "{program}");
    let boot = models.find("Rails::Railtie.run_initializers_of_railties(Rails.application, :boot)").unwrap_or_else(|| panic!("{models}"));
    assert!(root < load && load < boot, "{models}");

    let out = emit_and_run::ruby()
        .current_dir(&*dir)
        .args([
            "-e",
            // boot.rb's order, from the pieces this needs (the full boot
            // needs the pg gem): Rails, the railtie and engine ports,
            // app/models (which loads the engine and runs the :boot
            // initializers), then Finisher's.
            "ORDER = []\n\
             require_relative 'runtime/active_support_lazy_load_hooks'\n\
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
             puts ORDER.inspect\n\
             puts Foo::Engine.paths['test'].to_a == [File.expand_path('test')]\n\
             puts Dir.exist?('test') && $LOAD_PATH.include?(File.expand_path('test'))\n\
             puts Foo::Engine.root.to_s == Dir.pwd\n\
             puts Foo::Engine.config.foo.size\n\
             puts Rails::Engine.subclasses.inspect\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(
        stdout,
        "[\"foo.first\", \"foo.patches 4\", \"foo.second 3 X!\", \"to_prepare\", \"after_initialize\"]\n\
         true\n\
         true\n\
         true\n\
         4\n\
         [Foo::Engine]\n",
        "{stderr}"
    );
    assert!(
        stderr.contains("roundhouse: error[railtie_body_statement_not_run]: Foo::Engine body statement at lib/foo/engine.rb:14 did not run: NoMethodError"),
        "{stderr}"
    );
    assert!(
        stderr.contains("roundhouse: error[engine_glob_file_not_carried]: lib/foo/patches/notes.rb matches lib/foo/patches/**/*.rb in the source, but the tree carries no file for it; it was not loaded"),
        "{stderr}"
    );
    assert!(
        stderr.contains("roundhouse: error[engine_glob_file_not_loaded]: app/models/broken.rb (matched by lib/foo/patches/**/*.rb) did not load: ArgumentError"),
        "{stderr}"
    );
    assert!(stderr.contains("roundhouse: 1 railtie and engine initializer(s), hook(s) and body statement(s) did not run"), "{stderr}");
}

const ROUTED_ENGINE: &str = r#"require "rails"

module Dash
  class Engine < Rails::Engine
    engine_name "dash"
    routes do
      root "landings#show"
    end
    routes { get "x", to: "y#z" }
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql"]
fn an_engine_body_appends_its_routes_blocks_as_rails_does_and_boot_names_them() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_rails_engine_routes\n")
        .write("lib/dash/engine.rb", ROUTED_ENGINE)
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
             Rails.env_name = 'test'\n\
             require_relative 'runtime/rails_railtie'\n\
             require_relative 'runtime/rails_engine'\n\
             require_relative 'runtime/active_support_deprecation'\n\
             require_relative 'app/models'\n\
             Rails::Railtie.run_initializers_of_railties(Rails.application, :finish)\n\
             r = Dash::Engine.routes\n\
             puts r.class.name\n\
             puts r.instance_variable_get(:@append).size\n\
             puts Dash::Engine.instance.routes?.equal?(r)\n\
             puts [r.relative_url_root.inspect, r.api_only?.inspect, r.default_scope.inspect, r.draw_paths.inspect, r.default_url_options.inspect].join(' ')\n\
             puts Dash::Engine.config.route_set_class.name\n\
             puts Dash::Engine.routes.equal?(r)\n\
             begin\n\
               ActionDispatch::Routing::RouteSet.new.draw { root 'a#b' }\n\
             rescue NotImplementedError => e\n\
               puts e.message[/\\Aerror\\[route_set_not_drawn\\]/]\n\
             end\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(
        stdout,
        "ActionDispatch::Routing::RouteSet\n\
         2\n\
         true\n\
         nil false nil [] {}\n\
         ActionDispatch::Routing::RouteSet\n\
         true\n\
         error[route_set_not_drawn]\n",
        "{stderr}"
    );
    assert!(!stderr.contains("error[railtie_body_statement_not_run]: "), "{stderr}");
    let named = stderr
        .lines()
        .find(|l| l.contains("Dash::Engine initializer :add_routing_paths did not run: NotImplementedError: error[route_set_not_drawn]: the routes block at "))
        .unwrap_or_else(|| panic!("{stderr}"));
    assert!(named.contains("app/models/dash/engine.rb:"), "{named}");
    assert!(stderr.contains("roundhouse: 1 railtie and engine initializer(s), hook(s) and body statement(s) did not run"), "{stderr}");
}
