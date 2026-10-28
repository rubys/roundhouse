//! Constants a gem declares in a nested file are in the namespace that file
//! opens.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LOCK: &str = "PATH\n  remote: vendor/gems/probe_ident-0.5.0\n  specs:\n    probe_ident (0.5.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  probe_ident!\n";

const ENTRY: &str = r##"# frozen_string_literal: true

module ProbeIdent
end

require "probe_ident/config"
require "probe_ident/marker_identifier"
require "probe_ident/rails/bearer_authorization"
"##;

const CONFIG: &str = r##"# frozen_string_literal: true

module ProbeIdent
  VERSION = "0.5.0"

  class << self
    def test_mode?
      !!@test_mode
    end
  end
end
"##;

const MARKER_IDENTIFIER: &str = r##"# frozen_string_literal: true

module ProbeIdent
  class MarkerIdentifier
    SUPPORTED_MARKERS = [
      "atkn_2.",
      "atkn_",
      "stkn_",
    ].compact

    def initialize(token)
      @token = token
    end

    def marker
      SUPPORTED_MARKERS.find { |m| @token.to_s.start_with?(m) }
    end
  end
end
"##;

const BEARER: &str = r##"# frozen_string_literal: true

module ProbeIdent
  ACCESS_TOKEN_PATTERN = %r{[0-9a-zA-Z\-\.\_\~\+/]+}
  MARKER_PATTERN = /#{MarkerIdentifier::SUPPORTED_MARKERS.map { |marker| Regexp.escape(marker) }.join("|")}/
  MARKED_TOKEN_PATTERN = /(?<marker>#{MARKER_PATTERN})(?<token>#{ACCESS_TOKEN_PATTERN})/
  BEARER_AUTHORIZATION_PATTERN = /\ABearer\s(?<marked_token>#{MARKED_TOKEN_PATTERN})\z/

  module BearerAuthorization
    def self.marked_token(header)
      match = BEARER_AUTHORIZATION_PATTERN.match(header)
      match && match[:marked_token]
    end
  end
end
"##;

const APPLICATION: &str = "require_relative \"boot\"\n\nrequire \"rails\"\nrequire \"active_record/railtie\"\n\nBundler.require(*Rails.groups)\n\nmodule IdentProbeApp\n  class Application < Rails::Application\n  end\nend\n";

const CARRIED: &str = "test_paths:\n  - test\n";

const BEARER_TEST: &str = r##"require "test_helper"

class BearerAuthorizationTest < ActiveSupport::TestCase
  test "the namespace constants a nested file writes read what the gem required before it" do
    assert_equal "atkn_2.abc", ProbeIdent::BearerAuthorization.marked_token("Bearer atkn_2.abc")
    assert_nil ProbeIdent::BearerAuthorization.marked_token("Bearer zzz")
    assert_match ProbeIdent::MARKER_PATTERN, "stkn_x"
    assert_equal "0.5.0", ProbeIdent::VERSION
    assert_equal false, ProbeIdent.test_mode?
  end
end
"##;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", CARRIED)
        .write("Gemfile", "source \"https://rubygems.org\"\n\ngem \"activesupport\"\ngem \"probe_ident\", \"~> 0.5\"\n")
        .write("Gemfile.lock", LOCK)
        .write("vendor/gems/probe_ident-0.5.0/lib/probe_ident.rb", ENTRY)
        .write("vendor/gems/probe_ident-0.5.0/lib/probe_ident/config.rb", CONFIG)
        .write("vendor/gems/probe_ident-0.5.0/lib/probe_ident/marker_identifier.rb", MARKER_IDENTIFIER)
        .write("vendor/gems/probe_ident-0.5.0/lib/probe_ident/rails/bearer_authorization.rb", BEARER)
        .write("config/boot.rb", "require \"bundler/setup\"\n")
        .write("config/application.rb", APPLICATION)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("config/environment.rb", "require_relative \"application\"\n\nRails.application.initialize!\n")
        .write("test/models/bearer_authorization_test.rb", BEARER_TEST)
}

#[test]
fn namespace_constants_written_in_a_nested_file_load_with_that_file() {
    app().run_test("test/models/bearer_authorization_test.rb").assert_passes();
}
