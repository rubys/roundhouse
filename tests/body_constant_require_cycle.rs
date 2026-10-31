//! PostgreSQL: body constant require cycle, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const RESPONSE: &str = r#"module Sales
  module Query
    class AddResponse
      include Sales::Query::Changes
    end
  end
end
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const STRUCTURE: &str = r#"CREATE TABLE sales_changes (
    id bigserial NOT NULL,
    kind character varying,
    CONSTRAINT sales_changes_pkey PRIMARY KEY (id)
);
"#;

const MODEL_CHANGE: &str = r#"module Sales
  class Change < ActiveRecord::Base
    self.table_name = "sales_changes"

    ADD = "add"

    HANDLERS = { ADD => Sales::Query::AddResponse } #: Hash[String, singleton(Sales::Query::AddResponse)]
  end
end
"#;

const INTERFACE: &str = r#"module Typed
  #: (Hash[String, String] map) -> void
  def type_map=(map)
    @type_map = map
  end

  #: () -> Hash[String, String]
  def type_map
    @type_map || {}
  end
end
"#;

const TYPED_CHANGES: &str = r#"module Sales
  module Query
    module Changes
      extend(Typed)
      self.type_map = { Change::ADD => "added" }
    end
  end
end
"#;

/// A model's body constant names a class that includes a module, which
/// reads the model back through the lexical `Sales` scope: the require
/// waits for the model's class to open (inside its body, guarded), where
/// it was a circular autoload, a NameError at load.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_model_body_constant_reaching_a_module_that_reads_the_model_back_loads_in_the_body() {
    let (emitted, _errors) = emit_and_run::empty_app()
        .write("roundhouse.yml", "")
        .write("Gemfile.lock", LOCK)
        .write("config/routes.rb", "Rails.application.routes.draw do\nend\n")
        .write("db/structure.sql", STRUCTURE)
        .write("app/models/typed.rb", INTERFACE)
        .write("app/models/sales/change.rb", MODEL_CHANGE)
        .write("app/models/sales/query/changes.rb", TYPED_CHANGES)
        .write("app/models/sales/query/add_response.rb", RESPONSE)
        .emit(roundhouse::project::BuildTarget::Ruby);
    let text = std::fs::read_to_string(emitted.join("app/models/sales/change.rb")).unwrap();
    let guarded = "require_relative \"query/add_response\" unless defined?(::Sales::Query::AddResponse)";
    let class_at = text.find("class Change < ActiveRecord::Base").expect(&text);
    assert!(text.find(guarded).is_some_and(|at| at > class_at), "{text}");
    let module = std::fs::read_to_string(emitted.join("app/models/sales/query/changes.rb")).unwrap();
    assert!(module.contains("require_relative \"../change\" unless defined?(::Sales::Change)"), "{module}");
}
