//! A gem source and the gem it requires compile together, with their test
//! helpers.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const LOCK: &str = "PATH\n  remote: vendor/gems/eventrail-4.0.1\n  specs:\n    eventrail (4.0.1)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  stream-kit!\n  eventrail!\n";

const KAFKA_ENTRY: &str = "module StreamKit\n  VERSION = \"5.2.3\"\nend\n";

const KAFKA_V2: &str = "require \"stream-kit/eventrail/v2/eventrail\"\n";

const KAFKA_EVENTRAIL: &str = r#"module EventrailV2
  extend self

  def produced
    @produced ||= []
  end

  def produce(schema_id, payload)
    produced << [schema_id, payload]
    nil
  end
end
"#;

const KAFKA_TEST_HELPER: &str = r#"require "stream-kit/eventrail/v2/eventrail"

module EventrailV2
  module Assertions
    def assert_eventrail_produced(schema_id)
      assert EventrailV2.produced.any? { |schema, _| schema == schema_id }, "no #{schema_id} event produced"
    end
  end
end
"#;

const EVENTRAIL_ENTRY: &str = "require \"stream-kit/eventrail/v2\"\n\nObject.const_set(\"Eventrail\", EventrailV2)\n";

const EVENTRAIL_TEST_HELPER: &str = "require \"stream-kit/eventrail/v2/test_helper\"\n";

const SHITLIST: &str = "module Eventrail\n  class Shitlist\n    DEPRECATED = [\"old_event/1.0\"].freeze\n  end\nend\n";

const APPLICATION: &str = "require_relative \"boot\"\n\nrequire \"rails\"\nrequire \"active_record/railtie\"\n\nBundler.require(*Rails.groups)\n\nmodule EventrailProbeApp\n  class Application < Rails::Application\n  end\nend\n";

const CARRIED: &str = "test_paths:\n  - test\n";

/// The r5 symptom: the include was error[gem_requires_rails] at load.
const ASSERTIONS_TEST: &str = r#"require "test_helper"
require "eventrail/test_helper"

class AssertionsTest < ActiveSupport::TestCase
  include Eventrail::Assertions

  test "the gem's assertions through the Eventrail alias" do
    Eventrail.produce("checkout_created/1.0", { "id" => 1 })
    assert_eventrail_produced("checkout_created/1.0")
    assert_equal true, AssertionsTest.include?(EventrailV2::Assertions)
  end
end
"#;

const EVENT_TEST: &str = r#"require "test_helper"

class EventTest < ActiveSupport::TestCase
  test "the gem's alias is the boot's Eventrail, reopened by the app" do
    Eventrail.produce("checkout_created/1.0", { "id" => 1 })
    assert_equal [["checkout_created/1.0", { "id" => 1 }]], Eventrail.produced
    assert_same EventrailV2, Eventrail
    assert_equal ["old_event/1.0"], Eventrail::Shitlist::DEPRECATED
  end
end
"#;

const TEST_ENVIRONMENT: &str = "require \"eventrail/shitlist\"\n\nRails.application.configure do\n  config.eager_load = false\nend\n";

fn app(config: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", config)
        .write(
            "Gemfile",
            "source \"https://rubygems.org\"\n\ngem \"activesupport\"\ngem \"eventrail\", \"~> 4.0\"\ngem \"stream-kit\", \"~> 5.2.3\"\n",
        )
        .write("Gemfile.lock", LOCK)
        .write("vendor/gems/stream-kit-5.2.3/lib/stream-kit.rb", KAFKA_ENTRY)
        .write("vendor/gems/stream-kit-5.2.3/lib/stream-kit/eventrail/v2.rb", KAFKA_V2)
        .write("vendor/gems/stream-kit-5.2.3/lib/stream-kit/eventrail/v2/eventrail.rb", KAFKA_EVENTRAIL)
        .write("vendor/gems/stream-kit-5.2.3/lib/stream-kit/eventrail/v2/test_helper.rb", KAFKA_TEST_HELPER)
        .write("vendor/gems/eventrail-4.0.1/lib/eventrail.rb", EVENTRAIL_ENTRY)
        .write("vendor/gems/eventrail-4.0.1/lib/eventrail/test_helper.rb", EVENTRAIL_TEST_HELPER)
        .write("lib/eventrail/shitlist.rb", SHITLIST)
        .write("config/boot.rb", "require \"bundler/setup\"\n")
        .write("config/application.rb", APPLICATION)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("config/environment.rb", "require_relative \"application\"\n\nRails.application.initialize!\n")
        .write("config/environments/test.rb", TEST_ENVIRONMENT)
        .write("test/models/event_test.rb", EVENT_TEST)
        .write("test/models/assertions_test.rb", ASSERTIONS_TEST)
}

#[test]
fn a_gem_over_a_gem_source_carried_as_source_is_not_refused() {
    let (emitted, errors) = app(CARRIED).emit(BuildTarget::Ruby);
    assert!(!errors.iter().any(|e| e.contains("gem_requires_rails")), "{errors:#?}");
    let test = std::fs::read_to_string(emitted.join("test/models/assertions_test.rb")).expect("assertions_test.rb");
    assert!(!test.contains("gem_requires_rails"), "{test}");
    assert!(test.contains("include Eventrail::Assertions"), "{test}");
}

#[test]
#[ignore = "fork spec: gem closures"]
fn a_gem_over_a_gem_source_runs_its_alias_in_emitted_ruby() {
    app(CARRIED).run_test("test/models/event_test.rb").assert_passes();
}

#[test]
#[ignore = "fork spec: gem closures"]
fn a_gem_over_a_gem_source_aliases_without_a_constant_warning() {
    let run = app(CARRIED).run_test("test/models/event_test.rb");
    run.assert_passes();
    assert!(!run.stderr.contains("already initialized constant Eventrail"), "{}", run.stderr);
}

#[test]
#[ignore = "a test file's require of a compiled gem feature is not carried (Eventrail::Assertions uninitialized)"]
fn a_gem_over_a_gem_source_runs_its_assertions_in_emitted_ruby() {
    app(CARRIED).run_test("test/models/assertions_test.rb").assert_passes();
}
