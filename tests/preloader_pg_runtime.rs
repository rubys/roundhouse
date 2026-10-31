//! PostgreSQL: preloader, on the Ruby target against native Rails.
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
    CONSTRAINT shops_pkey PRIMARY KEY (id)
);

CREATE TABLE items (
    id bigserial NOT NULL,
    shop_id bigint,
    name character varying,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);

CREATE TABLE notes (
    id bigserial NOT NULL,
    item_id bigint,
    body character varying,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);

CREATE TABLE profiles (
    id bigserial NOT NULL,
    shop_id bigint,
    bio character varying,
    CONSTRAINT profiles_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_preloader
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
  has_many :items
  has_one :profile
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
  belongs_to :shop, optional: true
  has_many :notes
end
"#;

const NOTE: &str = r#"class Note < ApplicationRecord
  belongs_to :item, optional: true
end
"#;

const PROFILE: &str = r#"class Profile < ApplicationRecord
  belongs_to :shop, optional: true
end
"#;

const PROBE: &str = r##"class PreloaderProbe
  def self.names(records)
    records.map(&:name).sort.join(",")
  end

  def self.shops
    Shop.order(:id).to_a
  end

  def self.lines
    out = []
    a = Shop.create!(name: "a")
    b = Shop.create!(name: "b")
    Shop.create!(name: "c")
    a1 = Item.create!(shop_id: a.id, name: "a1")
    Item.create!(shop_id: a.id, name: "a2")
    Item.create!(shop_id: b.id, name: "b1")
    Note.create!(item_id: a1.id, body: "n1")
    Note.create!(item_id: a1.id, body: "n2")
    Profile.create!(shop_id: b.id, bio: "pb")

    s = shops
    ActiveRecord::Associations::Preloader.new(records: s, associations: :items).call
    out << "has_many: #{s.map { |x| x.association(:items).loaded? }.inspect} #{s.map { |x| names(x.items) }.inspect}"

    s = shops
    ActiveRecord::Associations::Preloader.new(records: s, associations: [:profile, { items: :notes }]).call
    loaded = s.map { |x| [x.association(:profile).loaded?, x.association(:items).loaded?] }
    notes = s.first.items.sort_by(&:name).map { |i| [i.association(:notes).loaded?, i.notes.map(&:body).sort] }
    out << "nested: #{loaded.inspect} #{s.map { |x| x.profile&.bio }.inspect} #{notes.inspect}"

    items = Item.order(:id).to_a
    ActiveRecord::Associations::Preloader.new(records: items, associations: { shop: :profile }).call
    out << "belongs_to: #{items.map { |i| [i.association(:shop).loaded?, i.shop.name, i.shop.association(:profile).loaded?] }.inspect}"

    s = shops
    s.first.association(:items).target = [Item.new(name: "kept")]
    ActiveRecord::Associations::Preloader.new(records: s, associations: :items).call
    out << "loaded kept: #{s.map { |x| names(x.items) }.inspect}"

    s = shops
    s.first.association(:items).target << Item.new(name: "unsaved")
    ActiveRecord::Associations::Preloader.new(records: s, associations: :items).call
    out << "unsaved kept: #{s.map { |x| names(x.association(:items).target) }.inspect}"

    s = shops
    ActiveRecord::Associations::Preloader.new(records: s, associations: :items, scope: Item.where(name: %w[a2 b1])).call
    out << "scope: #{s.map { |x| x.association(:items).loaded? }.inspect} #{s.map { |x| names(x.items) }.inspect}"

    items = Item.order(:id).to_a
    ActiveRecord::Associations::Preloader.new(records: items, associations: :shop, scope: Shop.where(name: "a")).call
    out << "scope belongs_to: #{items.map { |i| i.shop&.name }.inspect}"

    s = shops
    begin
      ActiveRecord::Associations::Preloader.new(records: s, associations: { items: :notes }, scope: Note.where(body: "n2")).call
      out << "scope nested: #{s.map { |x| names(x.items) }.inspect} #{s.first.items.sort_by(&:name).map { |i| i.notes.map(&:body) }.inspect}"
    rescue StandardError => e
      out << "scope nested: #{e.class}"
    end

    ActiveRecord::Associations::Preloader.new(records: [], associations: :items).call
    out << "empty: ok"

    begin
      ActiveRecord::Associations::Preloader.new(records: shops, associations: :nope).call
      out << "unknown: none"
    rescue StandardError => e
      out << "unknown: #{e.class}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 10] = [
    r#"has_many: [true, true, true] ["a1,a2", "b1", ""]"#,
    r#"nested: [[true, true], [true, true], [true, true]] [nil, "pb", nil] [[true, ["n1", "n2"]], [true, []]]"#,
    r#"belongs_to: [[true, "a", true], [true, "a", true], [true, "b", true]]"#,
    r#"loaded kept: ["kept", "b1", ""]"#,
    r#"unsaved kept: ["a1,a2,unsaved", "b1", ""]"#,
    r#"scope: [true, true, true] ["a2", "b1", ""]"#,
    r#"scope belongs_to: ["a", "a", nil]"#,
    r#"scope nested: ActiveRecord::StatementInvalid"#,
    r#"empty: ok"#,
    r#"unknown: ActiveRecord::AssociationNotFoundError"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/shop",
    "app/models/item",
    "app/models/note",
    "app/models/profile",
    "app/models/preloader_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/item.rb", ITEM),
    ("app/models/note.rb", NOTE),
    ("app/models/profile.rb", PROFILE),
    ("app/models/preloader_probe.rb", PROBE),
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
fn preloader_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "preloader",
        &FILES,
        &emitted,
        "rh_spec_preloader",
        "PreloaderProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
