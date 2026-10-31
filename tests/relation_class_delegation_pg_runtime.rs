//! PostgreSQL: relation class delegation, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE orders (
    id bigserial NOT NULL,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);

CREATE TABLE lines (
    id bigserial NOT NULL,
    order_id bigint,
    sku character varying,
    CONSTRAINT lines_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_relation_class_delegation
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const EVENT_LOG: &str = r#"class EventLog
  def self.lines
    @lines ||= []
  end
end
"#;

/// The prepended guard: its `ClassMethods` land on the model's singleton.
const MUTATION_GUARD: &str = r##"module MutationGuard
  module ClassMethods
    def record_event(model, destroyed: false)
      EventLog.lines << "#{model} destroyed=#{destroyed}"
    end

    def guarded_sku?(sku)
      sku.to_s.start_with?("g-")
    end

    def count_in_scope
      all.count
    end
  end

  def self.prepended(base)
    base.singleton_class.prepend(ClassMethods)
  end
end
"##;

/// The `extending` module: `self` is the relation, `name`,
/// `record_event` and `guarded_sku?` are the model's.
const GUARD_RELATION: &str = r#"module MutationGuard
  module Relation
    def delete_all(*args)
      record_event(name, destroyed: true)
      super
    end

    def update(*args)
      record_event(name) if guarded_sku?(args.last[:sku])
      super
    end

    def update_all(changes, options = {})
      record_event(name) if guarded_sku?(changes[:sku])
      super(changes)
    end
  end
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  has_many :lines, dependent: :delete_all
end
"#;

const LINE: &str = r#"class Line < ApplicationRecord
  prepend MutationGuard
  default_scope { extending(MutationGuard::Relation) }

  def self.by_sku(sku)
    where(sku: sku)
  end

  def self.tagged_count
    count_in_scope
  end
end
"#;

const PROBE: &str = r##"class RelationClassDelegationProbe
  def self.rel(order)
    Line.where(order_id: order.id)
  end

  def self.lines
    out = []
    order = Order.create!
    other = Order.create!
    order.lines.create!(sku: "g-1")
    order.lines.create!(sku: "g-2")
    order.lines.create!(sku: "x-3")
    other.lines.create!(sku: "g-4")

    out << "name: #{rel(order).name} #{order.lines.name}"
    out << "respond: #{rel(order).respond_to?(:name)} #{rel(order).respond_to?(:guarded_sku?)} #{rel(order).respond_to?(:no_such_class_method)}"
    out << "class method: #{rel(order).guarded_sku?("g-9")} #{rel(order).by_sku("g-1").count} #{order.lines.by_sku("g-2").count}"
    out << "scoped: #{rel(order).tagged_count} #{rel(order).by_sku("g-1").tagged_count} #{order.lines.tagged_count} #{Line.tagged_count}"
    begin
      rel(order).no_such_class_method
    rescue NoMethodError => e
      out << "missing: #{e.class} #{e.name.inspect}"
    end
    rel(order).where(sku: "x-3").update_all(sku: "g-3")
    out << "update_all: #{EventLog.lines.inspect}"
    EventLog.lines.clear
    rel(order).where(sku: "g-3").update(sku: "g-30")
    one = rel(order).update(Line.where(sku: "g-2").first.id, sku: "g-20")
    out << "update: #{EventLog.lines.inspect} #{one.sku} #{Line.where(order_id: order.id).order(:id).pluck(:sku).inspect}"
    EventLog.lines.clear
    order.lines.where(sku: "g-1").delete_all
    out << "relation delete_all: #{EventLog.lines.inspect} #{Line.count}"
    EventLog.lines.clear
    order.lines.delete_all
    out << "proxy delete_all: #{EventLog.lines.uniq.inspect} #{Line.pluck(:sku).inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 9] = [
    r#"name: Line Line"#,
    r#"respond: true true false"#,
    r#"class method: true 1 1"#,
    r#"scoped: 3 1 3 4"#,
    r#"missing: NoMethodError :no_such_class_method"#,
    r#"update_all: ["Line destroyed=false"]"#,
    r#"update: ["Line destroyed=false", "Line destroyed=false"] g-20 ["g-1", "g-20", "g-30"]"#,
    r#"relation delete_all: ["Line destroyed=true"] 3"#,
    r#"proxy delete_all: ["Line destroyed=true"] ["g-4"]"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "app/models/application_record",
    "app/models/event_log",
    "app/models/mutation_guard",
    "app/models/mutation_guard/relation",
    "app/models/order",
    "app/models/line",
    "app/models/relation_class_delegation_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/event_log.rb", EVENT_LOG),
    ("app/models/mutation_guard.rb", MUTATION_GUARD),
    ("app/models/mutation_guard/relation.rb", GUARD_RELATION),
    ("app/models/order.rb", ORDER),
    ("app/models/line.rb", LINE),
];

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_relation_delegates_class_methods_like_native_rails_on_postgresql() {
    let app = FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
        .write("app/models/relation_class_delegation_probe.rb", PROBE);
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let files: Vec<(&str, &str)> = FILES
        .iter()
        .copied()
        .chain([("app/models/relation_class_delegation_probe.rb", PROBE)])
        .collect();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation-class-delegation",
        &files,
        &emitted,
        "rh_spec_relation_class_delegation",
        "RelationClassDelegationProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
