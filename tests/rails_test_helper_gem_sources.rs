//! A test helper from a gem compiled as source is the app's code: a test
//! class's `include` of it works.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/courier-3.1.0\n  specs:\n    courier (3.1.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  courier!\n";

const RBI: &str = "# typed: true\nmodule Courier\n  module TestHelper\n    def stub_courier_response(name); end\n  end\n  module Middleware\n    class Error < StandardError; end\n  end\nend\n";

const ENTRY: &str = "require \"active_support\"\nrequire \"courier/middleware/error\"\nrequire \"courier/test_helper\"\n\nmodule Courier\n  VERSION = \"3.1.0\"\nend\n";

const ERROR: &str = "module Courier\n  module Middleware\n    class Error < StandardError\n      def initialize(name)\n        super(\"courier request #{name} failed\")\n      end\n    end\n  end\nend\n";

const HELPER: &str = "module Courier\n  module TestHelper\n    def stub_courier_response(name)\n      @courier_stubs ||= {}\n      @courier_stubs[name] = \"stubbed #{name}\"\n    end\n\n    def courier_response(name)\n      stubs = @courier_stubs || {}\n      stubs.fetch(name) { raise Courier::Middleware::Error.new(name) }\n    end\n  end\nend\n";

const APPLICATION: &str = "require_relative \"boot\"\n\nrequire \"rails\"\nrequire \"active_record/railtie\"\n\nBundler.require(*Rails.groups)\n\nmodule CourierProbeApp\n  class Application < Rails::Application\n  end\nend\n";

const CARRIED: &str = "test_paths:\n  - test\n";

const STUB_TEST: &str = r#"require "test_helper"

class StubTest < ActiveSupport::TestCase
  include Courier::TestHelper

  test "the carried helper stubs and its error raises" do
    stub_courier_response("shop")
    assert_equal "stubbed shop", courier_response("shop")
    error = assert_raises(Courier::Middleware::Error) { courier_response("order") }
    assert_equal "courier request order failed", error.message
    assert_equal true, StubTest.include?(Courier::TestHelper)
  end
end
"#;

fn app(config: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", config)
        .write(
            "Gemfile",
            "source \"https://rubygems.org\"\n\ngem \"activesupport\"\ngem \"courier\"\n",
        )
        .write("Gemfile.lock", LOCK)
        .write("sorbet/rbi/gems/courier@3.1.0.rbi", RBI)
        .write("vendor/gems/courier-3.1.0/lib/courier.rb", ENTRY)
        .write(
            "vendor/gems/courier-3.1.0/lib/courier/middleware/error.rb",
            ERROR,
        )
        .write("vendor/gems/courier-3.1.0/lib/courier/test_helper.rb", HELPER)
        .write("config/boot.rb", "require \"bundler/setup\"\n")
        .write("config/application.rb", APPLICATION)
        .write(
            "config/routes.rb",
            "Rails.application.routes.draw do\nend\n",
        )
        .write(
            "test/test_helper.rb",
            "require \"active_support/test_case\"\n",
        )
        .write("test/models/stub_test.rb", STUB_TEST)
}

#[test]
fn a_carried_rails_dependent_gem_s_test_helper_include_is_not_refused() {
    let (emitted, errors) = app(CARRIED).emit(BuildTarget::Ruby);
    assert!(
        !errors.iter().any(|e| e.contains("gem_requires_rails")),
        "{errors:#?}"
    );
    let test =
        std::fs::read_to_string(emitted.join("test/models/stub_test.rb")).expect("stub_test.rb");
    assert!(!test.contains("gem_requires_rails"), "{test}");
    assert!(test.contains("Courier::TestHelper"), "{test}");
}

#[test]
fn a_carried_rails_dependent_gem_s_test_helper_runs_in_emitted_ruby() {
    app(CARRIED)
        .run_test("test/models/stub_test.rb")
        .assert_passes();
}
