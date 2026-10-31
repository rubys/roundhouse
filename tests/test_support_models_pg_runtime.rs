//! PostgreSQL: test support models, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_support_models
"#;

const STRUCTURE: &str = "CREATE TABLE catalogs (\n    id bigserial NOT NULL,\n    shop_id bigint,\n    title character varying,\n    CONSTRAINT catalogs_pkey PRIMARY KEY (id)\n);\n";

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const REGISTRY: &str = r#"class ResponderRegistry
  def self.types
    @types ||= []
  end

  def self.register(type)
    types << type
  end
end
"#;

/// The helper's shape: a namespace module holding models over an existing
/// table (`self.table_name`), a class nested in a model, and the
/// responder types that name the models.
const HELPER: &str = r#"# frozen_string_literal: true

module ResponderHelper
  class TestModel < ApplicationRecord
    self.table_name = "catalogs" # a dummy model: any table with a shop_id column

    validates :title, presence: true

    class << self
      def filter_prioritized(responders)
        responders
      end
    end

    class IDGenerator
      def initialize(id_count, shop_id: nil)
        query = TestModel.order(:id)
        query = query.where(shop_id: shop_id) unless shop_id.nil?
        @ids = query.limit(id_count).ids
      end

      def next_id
        @ids.pop
      end
    end
  end

  class TestType
    class << self
      def name = "test_responder"

      def model = TestModel
    end
  end

  class TestSecondModel < ApplicationRecord
    self.table_name = "catalogs"

    class IDGenerator
      def initialize(id_count)
        @ids = TestSecondModel.order(:id).limit(id_count).ids
      end

      def next_id
        @ids.pop
      end
    end
  end

  class TestSecondType
    class << self
      def name = "test_responder_2"

      def model = TestSecondModel
    end
  end
end
"#;

const INITIALIZER: &str = r#"if Rails.env.test?
  ActiveSupport.on_load(:responder_registry) do
    require "support/helpers/responder_helper"

    ResponderRegistry.register(ResponderHelper::TestType)
    ResponderRegistry.register(ResponderHelper::TestSecondType)
  end
end
"#;

/// Fires the hook, as the registry's own load does, then reads the models.
const PROBE: &str = r##"class SupportModelsProbe
  def self.lines
    ActiveSupport.run_load_hooks(:responder_registry)
    first = ResponderHelper::TestType.model
    second = ResponderHelper::TestSecondType.model
    first.create!(title: "a", shop_id: 7)
    first.create!(title: "b", shop_id: 8)
    invalid = first.new
    [
      "registered: #{ResponderRegistry.types.map(&:name).inspect}",
      "models: #{first.name} #{second.name}",
      "tables: #{first.table_name} #{second.table_name}",
      "count: #{first.count} #{second.count}",
      "titles: #{second.order(:id).pluck(:title).inspect}",
      "valid: #{invalid.valid?} #{invalid.errors.full_messages.inspect}",
      "ids: #{ResponderHelper::TestModel::IDGenerator.new(2).next_id} #{ResponderHelper::TestModel::IDGenerator.new(2, shop_id: 7).next_id} #{ResponderHelper::TestSecondModel::IDGenerator.new(1).next_id}",
      "filter: #{first.filter_prioritized([1, 2]).inspect}"
    ]
  end
end
"##;

/// Native Rails: the test environment's `$LOAD_PATH` has `test`, and the
/// app's files load in their order.
const NATIVE_BOOT: &str = r#"$LOAD_PATH.unshift(File.join(__dir__, "test"))
require File.join(__dir__, "app/models/application_record")
require File.join(__dir__, "app/models/responder_registry")
require File.join(__dir__, "app/models/support_models_probe")
Rails = Struct.new(:env).new(Struct.new(:x) { def test? = true }.new)
load File.join(__dir__, "config/initializers/responders.rb")
"#;

const EXPECTED: [&str; 8] = [
    r#"registered: ["test_responder", "test_responder_2"]"#,
    "models: ResponderHelper::TestModel ResponderHelper::TestSecondModel",
    "tables: catalogs catalogs",
    "count: 2 2",
    r#"titles: ["a", "b"]"#,
    r#"valid: false ["Title can't be blank"]"#,
    "ids: 2 1 1",
    "filter: [1, 2]",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", "test_paths:\n  - test/support/helpers\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/responder_registry.rb", REGISTRY),
    ("app/models/support_models_probe.rb", PROBE),
    ("config/initializers/responders.rb", INITIALIZER),
    ("test/support/helpers/responder_helper.rb", HELPER),
    ("test/models/responder_test.rb", "require \"test_helper\"\nrequire \"support/helpers/responder_helper\"\n\nclass ResponderTest < ActiveSupport::TestCase\n  test \"models\" do\n    assert ResponderHelper::TestModel\n  end\nend\n"),
];

#[test]
#[ignore = "fork spec: postgresql"]
fn models_of_a_test_support_file_load_from_an_on_load_hook_like_native() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    // Both models are models of the emitted app, the namespace a module.
    for path in ["app/models/responder_helper/test_model.rb", "app/models/responder_helper/test_second_model.rb"] {
        let model = std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(model.contains("ApplicationRecord"), "{path}: {model}");
    }
    let native_files: Vec<(&str, &str)> = FILES.iter().copied().chain([("native_boot.rb", NATIVE_BOOT)]).collect();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "support-models",
        &native_files,
        &emitted,
        "rh_spec_support_models",
        "SupportModelsProbe",
        &["native_boot"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
