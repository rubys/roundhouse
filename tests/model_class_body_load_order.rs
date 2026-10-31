//! PostgreSQL: model class body load order, on the Ruby target against native Rails.
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
    legacy character varying DEFAULT 'db' NOT NULL,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
CREATE TABLE reports (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT reports_pkey PRIMARY KEY (id)
);
CREATE TABLE baskets (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT baskets_pkey PRIMARY KEY (id)
);
CREATE TABLE items (
    id bigserial NOT NULL,
    basket_id bigint,
    label character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_class_body_load_order
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// Only the `+=` form: no `self.ignored_columns = ..` call ahead of it.
const WIDGET: &str = r#"class Widget < ApplicationRecord
  self.ignored_columns += ["legacy"]
end
"#;

/// A class macro reading `table_name`, written above an eager constant.
const TABLE_RECORDING: &str = r#"module TableRecording
  def self.included(base)
    base.extend(ClassMethods)
  end

  module ClassMethods
    def record_table
      @recorded_table = table_name
    end

    def recorded_table
      @recorded_table
    end
  end
end
"#;

/// The `class << self` spelling; its `define_singleton_method` stays in
/// the hook and runs as written. (Not `Named`: inside a model that
/// constant is the ancestor `ActiveRecord::Scoping::Named`.)
const REQUIRES_NAME: &str = r#"module RequiresName
  class << self
    def included(base)
      base.validates :name, presence: true
      base.define_singleton_method(:named?) { true }
    end
  end
end
"#;

const REPORT: &str = r#"class Report < ApplicationRecord
  include TableRecording
  include RequiresName

  record_table

  LIMIT = 3
end
"#;

/// `has_many` is the includer's declaration; `before_save` stays in the
/// hook, which the PostgreSQL tree's runtime callbacks register.
const HAS_ITEMS: &str = r#"module HasItems
  def self.included(base)
    base.has_many :items
    base.before_save :x
  end

  def x
    self.name = "saved:#{name}"
  end
end
"#;

const BASKET: &str = r#"class Basket < ApplicationRecord
  include HasItems
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  belongs_to :basket, optional: true
end
"#;

const PROBE: &str = r##"class LoadOrderProbe
  def self.lines
    out = []
    out << "widget: #{Widget.ignored_columns.inspect} #{Widget.create!(name: "w").has_attribute?(:legacy)}"
    out << "report: #{Report.recorded_table.inspect} #{Report.named?} #{Report.new.valid?} #{Report.new(name: "r").valid?}"
    b = Basket.create!(name: "b")
    Item.create!(basket_id: b.id, label: "i")
    out << "basket: #{b.name} #{Basket.find(b.id).items.map(&:label).inspect} #{Basket.new.items.to_a.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 3] = [
    r#"widget: ["legacy"] false"#,
    r#"report: "reports" true false true"#,
    r#"basket: saved:b ["i"] []"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 9] = [
    "app/models/application_record",
    "app/models/table_recording",
    "app/models/has_items",
    "app/models/requires_name",
    "app/models/widget",
    "app/models/report",
    "app/models/item",
    "app/models/basket",
    "app/models/load_order_probe",
];

const FILES: [(&str, &str); 13] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/table_recording.rb", TABLE_RECORDING),
    ("app/models/requires_name.rb", REQUIRES_NAME),
    ("app/models/report.rb", REPORT),
    ("app/models/has_items.rb", HAS_ITEMS),
    ("app/models/basket.rb", BASKET),
    ("app/models/item.rb", ITEM),
    ("app/models/load_order_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

fn at(text: &str, needle: &str) -> usize {
    text.find(needle).unwrap_or_else(|| panic!("missing {needle:?} in:\n{text}"))
}

/// The compiler side: the synthesized `table_name` and
/// `_compiled_ignored_columns` stand above the class-body statements
/// that reach them, and the plain hooks' model DSL is the includers'.
#[test]
#[ignore = "fork spec: postgresql"]
fn synthesized_schema_methods_precede_the_class_body_statements_that_read_them() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap();

    let widget = read("app/models/widget.rb");
    // `+=` emits expanded, as the writer call it is.
    let write = at(&widget, "  self.ignored_columns = self.ignored_columns + [\"legacy\"]\n");
    assert!(at(&widget, "  def self.table_name\n") < write, "{widget}");
    assert!(at(&widget, "  def self._compiled_ignored_columns\n") < write, "{widget}");

    let report = read("app/models/report.rb");
    let call = at(&report, "  record_table\n");
    assert!(at(&report, "  def self.table_name\n") < call, "{report}");

    // `base.has_many` / `base.validates` left the hooks; what else a
    // hook does (a callback, `define_singleton_method`) stays.
    let has_items = read("app/models/has_items.rb");
    assert!(!has_items.contains("has_many"), "{has_items}");
    assert!(has_items.contains("base.before_save :x"), "{has_items}");
    let basket = read("app/models/basket.rb");
    assert!(basket.contains("  def items\n"), "{basket}");
    let requires_name = read("app/models/requires_name.rb");
    assert!(!requires_name.contains("validates"), "{requires_name}");
    assert!(requires_name.contains("define_singleton_method(:named?)"), "{requires_name}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn class_bodies_load_and_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "class-body-load-order",
        &FILES,
        &emitted,
        "rh_spec_class_body_load_order",
        "LoadOrderProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}

mod gem_port_macro {
    use super::{at, emit_and_run, pg_probe, real_errors, APPLICATION_RECORD};

    const STRUCTURE: &str = r#"CREATE TABLE ledgers (
    id bigserial NOT NULL,
    account character varying,
    CONSTRAINT ledgers_pkey PRIMARY KEY (id)
);
"#;

    const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_class_body_load_order_doc
"#;

    const APPLICATION: &str = r#"require_relative "boot"
require "rails/all"
Bundler.require(*Rails.groups)

module DocOrderProbe
  class Application < Rails::Application
  end
end
"#;

    const GEMFILE: &str = "source \"https://rubygems.org\"\ngem \"rails\"\ngem \"database_documentation\", \"~> 3.0\"\n";

    const LEDGER: &str = r#"class Ledger < ApplicationRecord
  include DatabaseDocumentation::ModelMetadata::Metadata

  database_documentation do |dd|
    dd.field(:account, datatype: DatabaseDocumentation::Datatypes::OtherIdentifier, is_pii: true)
  end

  LIMIT = 3
end
"#;

    const PROBE: &str = r##"class DocProbe
  def self.lines
    ma = Ledger.model_annotations
    ["ledger: #{ma.table_name} #{ma.fields.keys.inspect} #{Ledger::LIMIT}"]
  end
end
"##;

    const NATIVE_GEM: &str = r#"gem "railties"
gem "zeitwerk"
$LOAD_PATH.unshift(File.join(Gem::Specification.find_by_name("database_documentation", "3.2.0").gem_dir, "lib"))
require "rails"
require "database_documentation"
"#;

    /// What the gem answers natively (checked on the task PostgreSQL).
    const EXPECTED: [&str; 1] = ["ledger: ledgers [:account] 3"];

    const LOADS: [&str; 4] = [
        "native/database_documentation_gem.rb",
        "app/models/application_record.rb",
        "app/models/ledger.rb",
        "app/models/doc_probe.rb",
    ];

    const FILES: [(&str, &str); 10] = [
        ("roundhouse.yml", ""),
        ("config/database.yml", DATABASE_YML),
        ("config/application.rb", APPLICATION),
        ("Gemfile", GEMFILE),
        ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
        ("db/structure.sql", STRUCTURE),
        ("app/models/application_record.rb", APPLICATION_RECORD),
        ("app/models/ledger.rb", LEDGER),
        ("app/models/doc_probe.rb", PROBE),
        ("native/database_documentation_gem.rb", NATIVE_GEM),
    ];

    fn overlay() -> emit_and_run::Overlay {
        FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
    }

    #[test]
    #[ignore = "fork spec: postgresql"]
    fn the_port_macro_runs_below_the_synthesized_table_name() {
        let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
        assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
        let ledger = std::fs::read_to_string(emitted.join("app/models/ledger.rb")).unwrap();
        let call = at(&ledger, "  database_documentation do |dd|");
        assert!(at(&ledger, "  def self.table_name\n") < call, "{ledger}");
        assert!(at(&ledger, "  LIMIT = 3\n") < call, "{ledger}");
    }

    #[test]
    #[ignore = "fork spec: postgresql"]
    fn the_port_macro_loads_and_matches_the_gem_on_postgresql() {
        let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
        assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
        let Some((native, emitted)) = pg_probe::native_and_emitted(
            "class-body-load-order-doc",
            &FILES,
            &emitted,
            "rh_spec_class_body_load_order_doc",
            "DocProbe",
            &LOADS,
        ) else { return };
        assert_eq!(native, EXPECTED, "native");
        assert_eq!(emitted, native, "emitted");
    }
}
