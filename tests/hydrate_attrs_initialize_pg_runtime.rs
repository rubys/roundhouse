//! PostgreSQL: hydrate attrs initialize, on the Ruby target against native Rails.
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
    type character varying,
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_hydrate_attrs_initialize
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// The hook is what makes the factories construct with the sentinel
/// (`constructs_with_hydrate_attrs`).
const SHOP: &str = r##"class Shop < ApplicationRecord
  attr_reader :hooked

  after_initialize { @hooked = true }
end
"##;

/// An STI subclass, as ThemeJsonTemplateUpdateDeferredAction is of
/// DeferredAction: hydration reaches it through its base's `find`.
const PREMIUM_SHOP: &str = r##"class PremiumShop < Shop
  attr_reader :theme

  def initialize(*attrs)
    @theme = attrs[0]&.delete(:theme)
    super
  end
end
"##;

const PROBE: &str = r##"class HydrateAttrsProbe
  def self.lines
    out = []
    s = PremiumShop.new(name: "a", theme: "dark")
    out << "new: #{s.theme.inspect} #{s.name.inspect}"
    s.save!
    t = Shop.find(s.id)
    out << "find: #{t.class} #{t.theme.inspect} #{t.name.inspect} #{t.persisted?} #{t.hooked.inspect}"
    out << "where: #{Shop.where(name: "a").to_a.map { |r| r.theme.inspect }.inspect}"
    out << "bare: #{PremiumShop.new.theme.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 4] = [
    r#"new: "dark" "a""#,
    r#"find: PremiumShop nil "a" true true"#,
    r#"where: ["nil"]"#,
    r#"bare: nil"#,
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/application.rb", "\n"),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/premium_shop.rb", PREMIUM_SHOP),
    ("app/models/hydrate_attrs_probe.rb", PROBE),
];

#[test]
#[ignore = "fork spec: postgresql"]
fn an_initialize_deleting_from_its_attrs_runs_for_a_hydrated_record_as_native_loads_it() {
    let app = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "hydrate-attrs-initialize",
        &FILES,
        &emitted,
        "rh_spec_hydrate_attrs_initialize",
        "HydrateAttrsProbe",
        &["app/models/application_record", "app/models/shop", "app/models/premium_shop", "app/models/hydrate_attrs_probe"],
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let own: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(own.is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
