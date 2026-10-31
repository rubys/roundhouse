//! PostgreSQL: ar base receiver dispatch, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE widgets (
    id bigserial NOT NULL,
    name character varying,
    shop_id bigint,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_ar_base_receiver
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const CONCERN: &str = r#"module Versioned
  extend ActiveSupport::Concern

  def version_row
    record = self #: as ActiveRecord::Base
    transaction = record.class.current_transaction
    [record.class.table_name, record.id, transaction.open?, record.class.primary_key, record.has_attribute?(:shop_id), record.has_attribute?(:missing)]
  end
end
"#;

const TABLE_OF: &str = r#"module TableOf
  #: (singleton(ActiveRecord::Base)) -> String
  def self.call(klass)
    klass.table_name
  end

  def self.ascribed(model)
    klass = model #: as singleton(ActiveRecord::Base)
    [klass.table_name, klass.primary_key, klass.type_for_attribute("name").type]
  end
end
"#;

const UNKNOWN: &str = r#"module Unversioned
  def unknown_class_api
    record = self #: as ActiveRecord::Base
    record.class.some_unknown_ar_api
  end

  def unknown_instance_api
    record = self #: as ActiveRecord::Base
    record.some_unknown_instance_api
  end

  def unknown_value_api(model)
    klass = model #: as singleton(ActiveRecord::Base)
    klass.some_unknown_value_api
  end
end
"#;

const PROBE: &str = r#"class VersionedProbe
  def self.lines
    widget = Widget.create!(name: "a", shop_id: 7)
    out = []
    out << "row: #{widget.version_row.inspect}"
    Widget.transaction { out << "in transaction: #{widget.version_row[2].inspect}" }
    out << "after: #{widget.version_row[2].inspect}"
    out << "table of: #{TableOf.call(Widget).inspect} #{TableOf.ascribed(Widget).inspect}"
    out
  end
end
"#;

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/concerns/versioned.rb", CONCERN),
    ("app/models/widget.rb", "class Widget < ApplicationRecord\n  include Versioned\nend\n"),
    ("app/models/table_of.rb", TABLE_OF),
    ("app/models/versioned_probe.rb", PROBE),
];

fn overlay(files: &[(&str, &str)]) -> emit_and_run::Overlay {
    files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

fn refusal(method: &str) -> String {
    format!("ActiveRecord::Base not supported (all targets): {method} lacks a shared runtime/lowering implementation")
}

#[test]
#[ignore = "fork spec: postgresql"]
fn base_typed_receivers_dispatch_what_the_postgresql_runtime_defines() {
    let mut files = FILES.to_vec();
    files.push(("app/models/concerns/unversioned.rb", UNKNOWN));
    files.push(("app/models/gadget.rb", "class Gadget < ApplicationRecord\n  self.table_name = \"widgets\"\n  include Unversioned\nend\n"));
    let (emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    let errors = real_errors(&errors);
    let concern = std::fs::read_to_string(emitted.join("app/models/versioned.rb")).expect("emitted concern");
    for send in [
        "transaction = record.class.current_transaction",
        "record.class.table_name",
        "record.id",
        "transaction.open?",
        "record.class.primary_key",
        "record.has_attribute?(:shop_id)",
    ] {
        assert!(concern.contains(send), "no plain {send:?} in:\n{concern}");
    }
    assert!(!concern.contains("not supported"), "{concern}");
    let table_of = std::fs::read_to_string(emitted.join("app/models/table_of.rb")).expect("emitted module");
    for send in ["klass.table_name", "klass.primary_key", "klass.type_for_attribute(\"name\")"] {
        assert!(table_of.contains(send), "no plain {send:?} in:\n{table_of}");
    }
    assert!(!table_of.contains("not supported"), "{table_of}");
    // Only the methods no runtime defines, each by name.
    let refused: Vec<&String> = errors.iter().copied().filter(|e| e.contains("ActiveRecord::Base not supported")).collect();
    assert_eq!(refused.len(), 3, "{errors:#?}");
    for method in ["some_unknown_ar_api", "some_unknown_instance_api", "some_unknown_value_api"] {
        assert!(refused.iter().any(|e| e.contains(&refusal(method))), "{method}: {refused:#?}");
    }
    assert_eq!(errors.len(), 3, "{errors:#?}");
    let gadget = std::fs::read_to_string(emitted.join("app/models/unversioned.rb")).expect("emitted concern");
    assert_eq!(gadget.matches("roundhouse: ActiveRecord::Base not supported").count(), 3, "{gadget}");
}

/// Without the PostgreSQL runtime nothing reads its Base: a method only
/// it defines stays the named gap. A class method the registry models
/// for every Ruby-family tree answers through `record.class` too.
#[test]
#[ignore = "fork spec: postgresql"]
fn base_typed_receivers_stay_named_gaps_without_the_postgresql_runtime() {
    const READER: &str = r#"module Versioned
  def version_row
    record = self #: as ActiveRecord::Base
    [record.class.current_transaction, record.class.type_for_attribute(:name)]
  end
end
"#;
    let files = [
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/schema.rb", "ActiveRecord::Schema.define(version: 1) do\n  create_table \"widgets\" do |t|\n    t.string \"name\"\n  end\nend\n"),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/concerns/versioned.rb", READER),
        ("app/models/widget.rb", "class Widget < ApplicationRecord\n  include Versioned\nend\n"),
    ];
    let (_emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    let refused: Vec<&String> = errors.iter().filter(|e| e.contains("ActiveRecord::Base not supported")).collect();
    assert_eq!(refused.len(), 1, "{errors:#?}");
    assert!(refused[0].contains(&refusal("current_transaction")), "{refused:#?}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn base_typed_receivers_answer_as_natively_on_postgresql() {
    let (emitted, errors) = overlay(&FILES).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "ar-base-receiver",
        &FILES,
        &emitted,
        "rh_spec_ar_base_receiver",
        "VersionedProbe",
        &["app/models/application_record", "app/models/concerns/versioned", "app/models/widget", "app/models/table_of", "app/models/versioned_probe"],
    ) else { return };
    assert_eq!(
        native,
        [
            r#"row: ["widgets", 1, false, "id", true, false]"#,
            "in transaction: true",
            "after: false",
            r#"table of: "widgets" ["widgets", "id", :string]"#,
        ],
        "native"
    );
    assert_eq!(emitted_lines, native, "emitted");
}

/// graphql-kit's batch association loader asks `record.association(
/// name).loaded?` of a parameter typed `::ActiveRecord::Base`: every model
/// synthesizes `association`, so the send is a plain one. It was
/// refused by name (`ActiveRecord::Base not supported: association`),
/// which stubbed the call into a raise.
#[test]
#[ignore = "fork spec: postgresql"]
fn base_typed_receivers_answer_association() {
    const LOADER: &str = r#"class AssociationLoader
  #: (Symbol name) -> void
  def initialize(name)
    @name = name
  end

  #: (::ActiveRecord::Base record) -> bool
  def association_loaded?(record)
    record.association(@name).loaded?
  end
end
"#;
    let mut files = FILES.to_vec();
    files.push(("app/models/association_loader.rb", LOADER));
    let (emitted, errors) = overlay(&files).emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{errors:#?}");
    let loader = std::fs::read_to_string(emitted.join("app/models/association_loader.rb")).expect("emitted loader");
    assert!(loader.contains("record.association(@name).loaded?"), "{loader}");
    assert!(!loader.contains("not supported"), "{loader}");
}
