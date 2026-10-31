//! PostgreSQL: eager load join, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE countries (
    id bigserial NOT NULL,
    shop_id bigint,
    code character varying,
    CONSTRAINT countries_pkey PRIMARY KEY (id)
);
CREATE TABLE provinces (
    id bigserial NOT NULL,
    shop_id bigint,
    code character varying,
    CONSTRAINT provinces_pkey PRIMARY KEY (id)
);
CREATE TABLE tax_overrides (
    id bigserial NOT NULL,
    shop_id bigint,
    country_id bigint,
    is_not_deleted boolean DEFAULT true,
    CONSTRAINT tax_overrides_pkey PRIMARY KEY (id)
);
CREATE TABLE tax_override_regions (
    id bigserial NOT NULL,
    shop_id bigint,
    tax_override_id bigint,
    country_id bigint,
    province_id bigint,
    is_not_deleted boolean DEFAULT true,
    CONSTRAINT tax_override_regions_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_eager_join
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const COUNTRY: &str = "class Country < ApplicationRecord\nend\n";
const PROVINCE: &str = "class Province < ApplicationRecord\nend\n";

const TAX_OVERRIDE: &str = r#"class TaxOverride < ApplicationRecord
  belongs_to :country, optional: true
  has_many :tax_override_regions, inverse_of: :tax_override
end
"#;

const TAX_OVERRIDE_REGION: &str = r#"class TaxOverrideRegion < ApplicationRecord
  belongs_to :tax_override, inverse_of: :tax_override_regions
  belongs_to :country, optional: true
  belongs_to :province, optional: true
end
"#;

const PROBE: &str = r##"class EagerJoinProbe
  def self.regions(override)
    override.tax_override_regions.map { |r| "#{r.id}:#{r.country&.code.inspect}/#{r.province&.code.inspect}" }.sort.inspect
  end

  def self.lines
    out = []
    # The app asks for reflections, so the emitted models carry them (the
    # relation resolves a joined hop through them).
    out << "reflection: #{TaxOverride.reflect_on_association(:tax_override_regions).macro} #{TaxOverrideRegion.reflect_on_association(:country).macro}"
    ca = Country.create!(shop_id: 1, code: "CA")
    us = Country.create!(shop_id: 1, code: "US")
    other = Country.create!(shop_id: 2, code: "XX")
    qc = Province.create!(shop_id: 1, code: "QC")
    on = Province.create!(shop_id: 1, code: "ON")

    a = TaxOverride.create!(shop_id: 1, country_id: ca.id, is_not_deleted: true)
    b = TaxOverride.create!(shop_id: 1, country_id: us.id, is_not_deleted: true)
    c = TaxOverride.create!(shop_id: 1, country_id: ca.id, is_not_deleted: true)
    d = TaxOverride.create!(shop_id: 1, country_id: other.id, is_not_deleted: true)
    e = TaxOverride.create!(shop_id: 2, country_id: ca.id, is_not_deleted: true)
    a1 = TaxOverrideRegion.create!(shop_id: 1, tax_override_id: a.id, country_id: ca.id, is_not_deleted: true)
    a2 = TaxOverrideRegion.create!(shop_id: 1, tax_override_id: a.id, province_id: qc.id, is_not_deleted: true)
    a3 = TaxOverrideRegion.create!(shop_id: 1, tax_override_id: a.id, province_id: on.id, is_not_deleted: false)
    b1 = TaxOverrideRegion.create!(shop_id: 1, tax_override_id: b.id, country_id: us.id, is_not_deleted: true)
    TaxOverrideRegion.create!(shop_id: 1, tax_override_id: c.id, country_id: ca.id, is_not_deleted: false)
    TaxOverrideRegion.create!(shop_id: 1, tax_override_id: d.id, country_id: ca.id, is_not_deleted: true)
    TaxOverrideRegion.create!(shop_id: 2, tax_override_id: e.id, country_id: ca.id, is_not_deleted: true)

    # belongs_to hops, filtered on three of the joined tables, one of them
    # reached through an alias.
    found = TaxOverride.eager_load(:country, tax_override_regions: [:country, :province])
      .where(shop_id: 1)
      .where(countries: { shop_id: 1 })
      .where(tax_override_regions: { is_not_deleted: true })
      .where(is_not_deleted: true)
      .where("countries.id IS NOT NULL AND tax_override_regions.id IS NOT NULL")
      .where("tax_override_regions.province_id IS NOT NULL OR tax_override_regions.country_id IS NOT NULL")
      .where("countries_tax_override_regions.id IS NOT NULL OR provinces.id IS NOT NULL")
      .where("countries_tax_override_regions.shop_id = :shop_id OR provinces.shop_id = :shop_id", shop_id: 1)
      .to_a
    out << "fetch: #{found.map(&:id).sort == [a.id, b.id].sort}"
    out << "fetch size: #{found.size}"
    found.sort_by(&:id).each do |o|
      out << "fetch #{o.id == a.id ? "a" : "b"}: #{o.country.code} #{regions(o)}"
    end

    # The association holds the joined rows only: the deleted region is
    # filtered out of `a`'s collection although `a` has it.
    only = TaxOverride.eager_load(:tax_override_regions).where(tax_override_regions: { is_not_deleted: true }).where(id: a.id).to_a
    out << "filtered: #{only.size} #{only.first.tax_override_regions.map(&:id).sort == [a1.id, a2.id].sort}"
    out << "filtered loaded: #{only.first.tax_override_regions.loaded?}"

    # No WHERE: every parent once, a parent without children has an empty
    # loaded collection (LEFT OUTER JOIN).
    bare = TaxOverride.eager_load(:tax_override_regions).where(shop_id: 1).order(:id).to_a
    out << "bare: #{bare.map(&:id) == [a.id, b.id, c.id, d.id]} #{bare.map { |o| o.tax_override_regions.size }.inspect}"
    lonely = TaxOverride.create!(shop_id: 1, country_id: ca.id, is_not_deleted: true)
    out << "empty: #{TaxOverride.eager_load(:tax_override_regions).where(id: lonely.id).map { |o| [o.tax_override_regions.loaded?, o.tax_override_regions.size] }.inspect}"

    # `includes` plus a where on the collection's table is Rails'
    # `references` shape: JOINed, not preloaded.
    refd = TaxOverride.includes(:tax_override_regions).where(tax_override_regions: { is_not_deleted: false }).order(:id).to_a
    out << "includes+references: #{refd.map(&:id) == [a.id, c.id]} #{refd.map { |o| o.tax_override_regions.map(&:id).sort == [o.id == a.id ? a3.id : TaxOverrideRegion.where(tax_override_id: c.id).first.id] }.inspect}"

    # A second hop to a table already joined takes `<plural>_<parent>`.
    both = TaxOverride.eager_load(:country, tax_override_regions: :country)
      .where(countries: { code: "US" }).where(countries_tax_override_regions: { code: "US" }).to_a
    out << "alias: #{both.map(&:id) == [b.id]} #{both.first.country.code} #{both.first.tax_override_regions.map { |r| r.country.code }.inspect}"
    out << "sql alias: #{TaxOverride.eager_load(:country, tax_override_regions: :country).to_sql.include?("countries_tax_override_regions")}"
    out
  end
end
"##;

const EXPECTED: [&str; 12] = [
    "reflection: has_many belongs_to",
    "fetch: true",
    "fetch size: 2",
    r#"fetch a: CA ["1:\"CA\"/nil", "2:nil/\"QC\""]"#,
    r#"fetch b: US ["4:\"US\"/nil"]"#,
    "filtered: 1 true",
    "filtered loaded: true",
    "bare: true [3, 1, 1, 1]",
    "empty: [[true, 0]]",
    "includes+references: true [true, true]",
    r#"alias: true US ["US"]"#,
    "sql alias: true",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/country",
    "app/models/province",
    "app/models/tax_override",
    "app/models/tax_override_region",
    "app/models/eager_join_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/country.rb", COUNTRY),
    ("app/models/province.rb", PROVINCE),
    ("app/models/tax_override.rb", TAX_OVERRIDE),
    ("app/models/tax_override_region.rb", TAX_OVERRIDE_REGION),
    ("app/models/eager_join_probe.rb", PROBE),
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
fn eager_load_of_collections_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "eager-join",
        &FILES,
        &emitted,
        "rh_spec_eager_join",
        "EagerJoinProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
