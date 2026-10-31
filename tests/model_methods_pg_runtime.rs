//! PostgreSQL: model methods, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE shops (
    id bigserial NOT NULL,
    name character varying,
    number integer,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE themes (
    id bigserial NOT NULL,
    shop_id bigint,
    name character varying,
    updated_at timestamp(6) without time zone,
    CONSTRAINT themes_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_model_methods
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :themes, inverse_of: :shop

  def update_columns(changes)
    CallLog.add("update_columns #{changes.keys.inspect}")
    super
  end
end
"#;

const THEME: &str = r#"class Theme < ApplicationRecord
  belongs_to :shop, inverse_of: :themes
end
"#;

const CALL_LOG: &str = r#"class CallLog
  def self.lines
    @lines ||= []
  end

  def self.add(line)
    lines << line
  end
end
"#;

/// A namespace with its own `Theme`: a bare `Theme` inside it is this module.
const ONLINE_STORE_THEME: &str = r#"module OnlineStore
  module Theme
    def self.label
      "module"
    end
  end
end
"#;

const PROBE: &str = r##"module OnlineStore
  class ModelMethodsProbe
    def self.lines
      out = []
      shop = Shop.create!(name: "a", number: 1)
      out << "id_value: #{shop.id_value == shop.id} #{shop.id_value?}"
      fresh = Shop.new(name: "f")
      fresh.id_value = 4242
      out << "id_value=: #{fresh.id} #{Shop.attribute_aliases.inspect}"
      empty = Shop.create!(name: "e")
      begin
        empty.themes.first!
        out << "first!: no raise"
      rescue ActiveRecord::RecordNotFound => e
        out << "first!: #{e.class.name} #{e.model}"
      end
      t1 = shop.themes.create!(name: "one")
      t2 = shop.themes.create!(name: "two")
      out << "create!: #{t1.class.name} #{t1.shop_id == shop.id} #{Theme.label}"
      out << "bangs: #{shop.themes.first!.name} #{shop.themes.last!.name} #{shop.themes.take!.shop_id == shop.id}"
      owner = Shop.find(shop.id)
      found = owner&.themes&.where(name: "two")&.to_a || []
      none = nil
      missing = none&.themes&.where(name: "two")&.to_a || []
      out << "safe where: #{found.map(&:name).inspect} #{missing.inspect} #{t2.name}"
      shop.update_columns(name: "b", number: 2)
      out << "update_columns: #{shop.reload.name} #{shop.number}"
      shop.update_column(:name, "c")
      out << "update_column: #{Shop.find(shop.id).name} #{CallLog.lines.inspect}"
      before = ::Theme.find(t2.id).updated_at
      t2.update_columns(name: "zwei", touch: true)
      after = ::Theme.find(t2.id)
      out << "touch: #{after.name} #{!after.updated_at.nil? && after.updated_at != before}"
      begin
        Shop.new(name: "n").update_columns(name: "x")
      rescue ActiveRecord::ActiveRecordError => e
        out << "new record: #{e.message}"
      end
      out
    end
  end
end
"##;

const EXPECTED: [&str; 10] = [
    "id_value: true true",
    r#"id_value=: 4242 {"id_value" => "id"}"#,
    "first!: ActiveRecord::RecordNotFound Theme",
    "create!: Theme true module",
    "bangs: one two true",
    r#"safe where: ["two"] [] two"#,
    "update_columns: b 2",
    r#"update_column: c ["update_columns [:name, :number]", "update_columns [:name, :touch]"]"#,
    "touch: zwei true",
    "new record: cannot update a new record",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/call_log",
    "app/models/shop",
    "app/models/theme",
    "app/models/online_store/theme",
    "app/models/online_store/model_methods_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n"),
    ("app/models/call_log.rb", CALL_LOG),
    ("app/models/shop.rb", SHOP),
    ("app/models/theme.rb", THEME),
    ("app/models/online_store/theme.rb", ONLINE_STORE_THEME),
    ("app/models/online_store/model_methods_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn model_methods_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "model-methods",
        &FILES,
        &emitted,
        "rh_spec_model_methods",
        "OnlineStore::ModelMethodsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The emitted shape, without PostgreSQL: the `id_value` twins, the
/// rooted `::Theme` and the seeded relations.
#[test]
#[ignore = "fork spec: postgresql"]
fn model_methods_emitted_shape() {
    let (emitted, _errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let read = |rel: &str| std::fs::read_to_string(emitted.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let shop = read("app/models/shop.rb");
    for want in ["def id_value\n", "def id_value=(value)", "def id_value?"] {
        assert!(shop.contains(want), "missing `{want}`:\n{shop}");
    }
    let probe = read("app/models/online_store/model_methods_probe.rb");
    assert!(probe.contains("::Theme.create!("), "{probe}");
    assert!(!probe.contains(" Theme.create!("), "{probe}");
    assert!(probe.contains(".first!"), "{probe}");
    assert!(!probe.contains("themes.first!"), "first! left on the reader's Array:\n{probe}");
    assert!(!probe.contains("themes).where"), "&.where left on the reader's Array:\n{probe}");
}
