//! PostgreSQL: engine patch framework reopens, on the Ruby target against native Rails.
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

const REQUEST_PATCH: &str = r#"module RequestHelpers
  def helped?
    true
  end
end

module ActionDispatch
  class Request
    include RequestHelpers
  end
end
"#;

const TEST_CASE_HOOK: &str = r#"module RequestSession
  def session_label
    "label"
  end
end

ActiveSupport.on_load(:action_controller_test_case) do
  module ActionController
    class TestCase
      def session_label
        request.session_label
      end
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql layout"]
fn engine_patches_load_their_framework_reopens_and_hook_reopens_register_on_the_base() {
    let (dir, _) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("config/database.yml", "test:\n  adapter: postgresql\n  database: rh_engine_patch_reopens\n")
        .write("lib/foo/engine.rb", ENGINE)
        .write("lib/foo/patches/request.rb", REQUEST_PATCH)
        .write("lib/foo/patches/test_case_hook.rb", TEST_CASE_HOOK)
        .write("config/environments/test.rb", "Rails.application.configure do\n  config.eager_load = false\nend\n")
        .emit(BuildTarget::Ruby);
    let engine = std::fs::read_to_string(dir.join("app/models/foo/engine.rb")).expect("the engine is carried");
    assert!(
        engine.contains("RoundhouseAutoload.require_carried_files(\"lib/foo/patches/**/*.rb\", [\"models/action_dispatch/request\", \"models/request_helpers\", \"models/roundhouse_files/lib/foo/patches/test_case_hook\"], [])"),
        "{engine}"
    );
    let program = std::fs::read_to_string(dir.join("app/models/roundhouse_files/lib/foo/patches/test_case_hook.rb"))
        .expect("the hook's file program");
    assert!(program.contains("ActiveSupport.on_load(:action_controller_test_case) do\n"), "{program}");
    assert!(program.contains("def session_label\n"), "{program}");
    assert!(!program.contains("class TestCase"), "{program}");

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
             puts ActionDispatch::Request.include?(RequestHelpers)\n\
             module ActionController; class TestCase; def request; Struct.new(:session_label).new('from request'); end; end; end\n\
             puts ActionController::TestCase.method_defined?(:session_label)\n\
             ActiveSupport.run_load_hooks(:action_controller_test_case, ActionController::TestCase)\n\
             puts ActionController::TestCase.new.session_label\n",
        ])
        .output()
        .expect("ruby");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert_eq!(stdout, "true\nfalse\nfrom request\n", "{stderr}");
    assert!(!stderr.contains("error["), "{stderr}");
}
