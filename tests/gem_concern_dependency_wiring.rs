//! A gem Concern's dependencies are wired before the includer's class body
//! runs.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests, with the gem written
//! as a PATH gem of `Gemfile.lock`. A test the compiler does not pass yet
//! is ignored with the group of behavior it waits on (`fork spec: ...`).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const LOCK: &str = "PATH\n  remote: vendor/gems/miniredact-1.0.0\n  specs:\n    miniredact (1.0.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    activesupport (8.0.2)\n\nDEPENDENCIES\n  activesupport\n  miniredact!\n";

const SCHEMA: &str = "ActiveRecord::Schema[8.1].define(version: 2026_01_01_000000) do\n  create_table \"notes\", force: :cascade do |t|\n    t.string \"title\"\n  end\nend\n";

const GEM_ENTRY: &str = "require \"active_support\"\nrequire \"active_support/concern\"\nrequire \"active_record\"\nrequire \"miniredact/concerns/metadata\"\nrequire \"miniredact/concerns/redactable\"\nrequire \"miniredact/railtie\"\n";

const METADATA: &str = "module Miniredact\n  module Metadata\n    extend ActiveSupport::Concern\n\n    module ClassMethods\n      def documented = :documented\n    end\n  end\nend\n";

const REDACTABLE: &str = "module Miniredact\n  module Redactable\n    extend ActiveSupport::Concern\n\n    included do\n      class_attribute :redaction_strategies, default: {}\n    end\n\n    module ClassMethods\n      def custom_strategy(field, with:)\n        self.redaction_strategies = redaction_strategies.merge(field => with)\n      end\n    end\n\n    def redact_title = self.class.redaction_strategies.fetch(:title).call(self)\n  end\nend\n";

fn railtie(hook: &str) -> String {
    format!("require \"rails\"\nrequire \"rails/railtie\"\nrequire \"miniredact/concerns/metadata\"\nrequire \"miniredact/concerns/redactable\"\n\nmodule Miniredact\n  class Railtie < ::Rails::Railtie\n    config.eager_load_namespaces << Miniredact\n\n{hook}\n  end\nend\n")
}

const INITIALIZER: &str = "    initializer \"miniredact.attach\" do\n      Miniredact::Metadata.include Miniredact::Redactable\n    end";

const ON_LOAD: &str = "    initializer \"miniredact.attach\" do\n      ActiveSupport.on_load(:active_record) do\n        Miniredact::Metadata.include Miniredact::Redactable\n      end\n    end";

const SHARED: &str = "module Shared\n  extend ActiveSupport::Concern\n  include Miniredact::Metadata\n\n  included do\n    custom_strategy :title, with: proc { |r| \"redacted-#{r.id}\" }\n  end\nend\n";

const NOTE_TEST: &str = r#"require "test_helper"

class NoteTest < ActiveSupport::TestCase
  test "the outer concern's includers get the dependency's class side and instance side" do
    note = Note.create!(title: "secret")
    assert_equal :documented, Note.documented
    assert_equal [:title], Note.redaction_strategies.keys
    assert_equal "redacted-#{note.id}", note.redact_title
  end
end
"#;

fn app(railtie_body: &str) -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test\n")
        .write("Gemfile.lock", LOCK)
        .write("vendor/gems/miniredact-1.0.0/lib/miniredact.rb", GEM_ENTRY)
        .write("vendor/gems/miniredact-1.0.0/lib/miniredact/concerns/metadata.rb", METADATA)
        .write("vendor/gems/miniredact-1.0.0/lib/miniredact/concerns/redactable.rb", REDACTABLE)
        .write("vendor/gems/miniredact-1.0.0/lib/miniredact/railtie.rb", &railtie(railtie_body))
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/schema.rb", SCHEMA)
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("app/models/concerns/shared.rb", SHARED)
        .write("app/models/note.rb", "class Note < ApplicationRecord\n  include Shared\nend\n")
        .write("test/test_helper.rb", "require \"active_support/test_case\"\n")
        .write("test/models/note_test.rb", NOTE_TEST)
}

#[test]
fn a_railtie_initializer_s_concern_include_reaches_the_includers_of_the_outer_concern() {
    app(INITIALIZER).run_test("test/models/note_test.rb").assert_passes();
}

#[test]
fn an_on_load_block_s_concern_include_reaches_the_includers_of_the_outer_concern() {
    app(ON_LOAD).run_test("test/models/note_test.rb").assert_passes();
}
