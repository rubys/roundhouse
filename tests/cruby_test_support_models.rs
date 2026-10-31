//! PostgreSQL: cruby test support models, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

use roundhouse::project::BuildTarget;

const HELPER: &str = r#"# frozen_string_literal: true

require "set"

module ResponderHelper
  class TestModel < ApplicationRecord
    self.table_name = "catalogs"

    class IDGenerator
      def next_id = TestModel.order(:id).limit(1).ids.first
    end
  end

  class TestSecondModel < ApplicationRecord
    self.table_name = "catalogs"
  end

  class TestType
    class << self
      def model = TestModel
    end
  end
end
"#;

#[test]
#[ignore = "fork spec: postgresql layout"]
fn models_of_a_test_support_file_are_carried_with_the_file() {
    let (emitted, errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "test_paths:\n  - test/support/helpers\n")
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write(
            "db/structure.sql",
            "CREATE TABLE catalogs (\n    id bigserial NOT NULL,\n    title character varying,\n    CONSTRAINT catalogs_pkey PRIMARY KEY (id)\n);\n",
        )
        .write("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n")
        .write("test/support/helpers/responder_helper.rb", HELPER)
        .write(
            "test/models/responder_test.rb",
            "require \"test_helper\"\nrequire \"support/helpers/responder_helper\"\n\nclass ResponderTest < ActiveSupport::TestCase\n  test \"models\" do\n    assert ResponderHelper::TestType.model\n  end\nend\n",
        )
        .emit(BuildTarget::Ruby);
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    for model in ["test_model", "test_second_model"] {
        let text = read(&format!("app/models/responder_helper/{model}.rb"));
        assert!(text.contains("< ApplicationRecord"), "{model}: {text}");
        assert!(text.contains("\"catalogs\""), "{model}: {text}");
    }
    assert!(read("app/models/responder_helper/test_model/id_generator.rb").contains("class IDGenerator"));
    assert!(read("app/models/responder_helper/test_type.rb").contains("def self.model"));
    // The file's own `require` and each class it declares load with its feature.
    let program = read("app/models/roundhouse_files/test/support/helpers/responder_helper.rb");
    assert!(program.contains("require \"set\""), "{program}");
    assert!(program.contains("responder_helper/test_model\")"), "{program}");
    assert!(program.contains("responder_helper/test_second_model\")"), "{program}");
    let autoload = read("app/models.rb");
    assert!(
        autoload.contains(
            "\"support/helpers/responder_helper\" => [\"models/roundhouse_files/test/support/helpers/responder_helper\"]"
        ),
        "the test load path answers the helper's feature"
    );
}
