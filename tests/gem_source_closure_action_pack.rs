//! A gem that requires actionpack's root feature gets the framework
//! feature.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/paybox-3.0.1\n  specs:\n    paybox (3.0.1)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  paybox!\n";

const PAYBOX_ENTRY: &str = "require \"paybox/helper\"\nrequire \"paybox/view_helper\"\n";

const PAYBOX_HELPER: &str = r#"module Paybox
  class Helper
    def initialize(order)
      @order = order
    end

    def fields
      { "order" => @order }
    end
  end
end
"#;

const PAYBOX_VIEW_HELPER: &str = r#"require 'action_pack'
require "abstract_controller"
require "arel"

module Paybox
  ViewHelperError = Class.new(StandardError)

  module ViewHelper
    def payment_fields(order)
      raise ViewHelperError, "no order" if order.nil?
      Paybox::Helper.new(order).fields
    end
  end
end
"#;

const APPLICATION: &str = "require_relative \"boot\"\n\nrequire \"rails\"\nrequire \"active_record/railtie\"\n\nBundler.require(*Rails.groups)\n\nmodule PayboxProbeApp\n  class Application < Rails::Application\n  end\nend\n";

const CARRIED: &str = "test_paths:\n  - test\n";

const HELPER_TEST: &str = r#"require "test_helper"

class ViewHelperTest < ActiveSupport::TestCase
  include Paybox::ViewHelper

  test "the gem that requires action_pack loads at boot" do
    assert_equal({ "order" => 7 }, payment_fields(7))
    assert_raises(Paybox::ViewHelperError) { payment_fields(nil) }
  end
end
"#;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"activesupport\"\ngem \"paybox\", \"~> 3.0\"\n")
        .write("Gemfile.lock", LOCK)
        .write("vendor/gems/paybox-3.0.1/lib/paybox.rb", PAYBOX_ENTRY)
        .write("vendor/gems/paybox-3.0.1/lib/paybox/helper.rb", PAYBOX_HELPER)
        .write("vendor/gems/paybox-3.0.1/lib/paybox/view_helper.rb", PAYBOX_VIEW_HELPER)
        .write("config/boot.rb", "require \"bundler/setup\"\n")
        .write("config/application.rb", APPLICATION)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("config/environment.rb", "require_relative \"application\"\n\nRails.application.initialize!\n")
        .write("test/models/view_helper_test.rb", HELPER_TEST)
}

#[test]
#[ignore = "fork spec: gem closures"]
fn a_gem_sources_require_of_action_pack_is_a_framework_feature() {
    let (emitted, errors) = app().emit(BuildTarget::Ruby);
    assert!(errors.is_empty(), "{errors:#?}");
    let view_helper = std::fs::read_to_string(emitted.join("app/models/roundhouse_files/vendor/gems/paybox-3.0.1/lib/paybox/view_helper.rb"))
        .unwrap_or_default();
    for feature in ["action_pack", "abstract_controller", "arel"] {
        let required = format!("  require {feature:?}");
        assert!(!view_helper.lines().any(|line| line == required), "{view_helper}");
        assert!(view_helper.contains(&format!("`require {feature:?}` is a Rails feature this tree's runtime ports answer")), "{view_helper}");
    }
}

#[test]
fn a_gem_source_that_requires_action_pack_boots_in_emitted_ruby() {
    let run = app().run_test("test/models/view_helper_test.rb");
    run.assert_passes();
    assert!(!run.stderr.contains("file_statement_not_run"), "{}", run.stderr);
}
