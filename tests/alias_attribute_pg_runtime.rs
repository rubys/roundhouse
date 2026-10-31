//! PostgreSQL: alias attribute, on the Ruby target against native Rails.
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
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_alias_attribute
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true

  def self.aliases_column(new_name, column)
    alias_attribute new_name, column
  end

  def self.renames(new_name, column)
    CallLog.add("renames #{new_name}")
    alias_attribute(new_name, column)
  end
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

/// The aliases a replayed macro declares.
const STORE: &str = r#"class Store < ApplicationRecord
  self.table_name = "shops"
  aliases_column :handle, :name
  renames "count", "number"

  def handle_renamed?
    handle_changed?
  end
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  alias_attribute :subdomain, :name
  alias_attribute(:tracking_number, :number)

  def renamed?
    subdomain_changed?
  end

  def old_subdomain
    subdomain_was
  end
end
"#;

const PROBE: &str = r##"class AliasAttributeProbe
  def self.lines
    out = []
    s = Shop.new(subdomain: "a", tracking_number: 7)
    out << "new: #{s.name.inspect} #{s.subdomain.inspect} #{s.subdomain?} #{s.tracking_number.inspect} #{s.number.inspect}"
    s.subdomain = "b"
    out << "write: #{s.name.inspect} #{s.renamed?} #{s.old_subdomain.inspect}"
    s.save!
    out << "saved: #{s.saved_change_to_subdomain?} #{s.subdomain_previously_changed?} #{s.subdomain_previously_was.inspect} #{Shop.pluck(:name).inspect}"
    Shop.create!(subdomain: "c", tracking_number: 3)
    out << "where: #{Shop.where(subdomain: "b").count} #{Shop.find_by(subdomain: "c").number} #{Shop.where(tracking_number: 3).pluck(:subdomain).inspect} #{Shop.exists?(subdomain: "c")}"
    out << "order: #{Shop.order(subdomain: :desc).pluck(:name).inspect} #{Shop.order(:tracking_number).pluck(:tracking_number).inspect}"
    s.update!(subdomain: "d")
    out << "update: #{s.reload.name.inspect} #{Shop.where(name: "d").count}"
    out << "read: #{s.read_attribute(:subdomain).inspect} #{s[:subdomain].inspect}"
    out << "class: #{Shop.attribute_aliases.inspect} #{Shop.attribute_alias?(:subdomain)} #{Shop.attribute_alias(:subdomain).inspect}"
    st = Store.create!(handle: "m", count: 5)
    st.handle = "n"
    out << "macro: #{st.name.inspect} #{st.handle_renamed?} #{Store.where(handle: "m").count} #{Store.find_by(count: 5).handle.inspect} #{Store.order(handle: :desc).pluck(:handle).first.inspect}"
    out << "macro class: #{Store.attribute_aliases.inspect} #{CallLog.lines.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 10] = [
    r#"new: "a" "a" true 7 7"#,
    r#"write: "b" true nil"#,
    r#"saved: true true nil ["b"]"#,
    r#"where: 1 3 ["c"] true"#,
    r#"order: ["c", "b"] [3, 7]"#,
    r#"update: "d" 1"#,
    r#"read: "d" "d""#,
    r#"class: {"subdomain" => "name", "tracking_number" => "number", "id_value" => "id"} true "name""#,
    r#"macro: "n" true 1 "m" "m""#,
    r#"macro class: {"handle" => "name", "count" => "number", "id_value" => "id"} ["renames count"]"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/call_log",
    "app/models/shop",
    "app/models/store",
    "app/models/alias_attribute_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/call_log.rb", CALL_LOG),
    ("app/models/shop.rb", SHOP),
    ("app/models/store.rb", STORE),
    ("app/models/alias_attribute_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn alias_attribute_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "alias-attribute",
        &FILES,
        &emitted,
        "rh_spec_alias_attribute",
        "AliasAttributeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// A macro that reaches `alias_attribute` under a branch declares
/// nothing statically, and its call stays refused.
#[test]
#[ignore = "fork spec: postgresql"]
fn a_macro_alias_under_a_branch_stays_refused() {
    let app = overlay().write(
        "app/models/store.rb",
        "class Store < ApplicationRecord\n  self.table_name = \"shops\"\n  def self.maybe(n, c)\n    alias_attribute(n, c) if n\n  end\n  maybe :handle, :name\nend\n",
    );
    let (_dir, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("maybe not supported") && e.contains("alias_attribute")),
        "{}",
        errors.join("\n")
    );
}
