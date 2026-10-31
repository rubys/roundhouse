//! PostgreSQL: sti subclass macros, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE saved_searches (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT saved_searches_pkey PRIMARY KEY (id)
);

CREATE TABLE saved_views (
    id bigserial NOT NULL,
    type character varying,
    name character varying,
    saved_search_id bigint,
    pinned_by_id bigint,
    CONSTRAINT saved_views_pkey PRIMARY KEY (id)
);

CREATE TABLE pins (
    id bigserial NOT NULL,
    saved_view_id bigint,
    label character varying,
    CONSTRAINT pins_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_sti_subclass_macros
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  self.belongs_to_required_by_default = true
end
"#;

const SAVED_SEARCH: &str = r#"class SavedSearch < ApplicationRecord
end
"#;

const SAVED_VIEW: &str = r#"class SavedView < ApplicationRecord
end
"#;

const ORDER_SAVED_VIEW: &str = r#"class OrderSavedView < SavedView
  belongs_to :saved_search
  belongs_to :pinned_by, class_name: "SavedSearch", optional: true
  has_many :pins, foreign_key: :saved_view_id, dependent: :destroy
  has_one :first_pin, -> { order(:id) }, class_name: "Pin", foreign_key: :saved_view_id

  scope :named, ->(name) { where(name: name) }
  scope :searched, -> { where.not(saved_search_id: nil) }
  scope :for_search, ->(search:, name: nil) { name ? where(saved_search_id: search.id, name: name) : where(saved_search_id: search.id) }
end
"#;

const OTHER_SAVED_VIEW: &str = r#"class OtherSavedView < SavedView
end
"#;

const PIN: &str = r#"class Pin < ApplicationRecord
  belongs_to :saved_view
end
"#;

const PROBE: &str = r##"class StiSubclassMacrosProbe
  def self.lines
    out = []
    s = SavedSearch.create!(name: "s")
    t = SavedSearch.create!(name: "t")
    a = OrderSavedView.create!(name: "a", saved_search: s)
    b = OrderSavedView.new(name: "b")
    b.saved_search = t
    b.save!
    bare = OrderSavedView.new(name: "c")
    OtherSavedView.create!(name: "a")
    out << "belongs_to: #{a.saved_search.name} #{b.saved_search_id == t.id} #{OrderSavedView.find(b.id).saved_search.name} #{bare.valid?} #{bare.errors.full_messages.join("|")} #{OrderSavedView.new(name: "d").saved_search.inspect}"
    Pin.create!(saved_view: a, label: "p2")
    Pin.create!(saved_view: a, label: "p1")
    out << "has_many: #{a.pins.count} #{a.pins.order(:label).map(&:label).join(",")} #{OrderSavedView.find(a.id).pins.size} #{b.pins.to_a.size} #{a.first_pin.label} #{b.first_pin.inspect}"
    out << "scope: #{OrderSavedView.named("a").count} #{OrderSavedView.searched.order(:id).map(&:name).join(",")} #{OrderSavedView.where(id: [a.id, b.id]).named("b").map(&:name).join(",")} #{OrderSavedView.for_search(search: s).map(&:name).join(",")} #{OrderSavedView.for_search(search: t, name: "x").count} #{SavedView.where(name: "a").count}"
    a.destroy
    out << "dependent: #{Pin.count} #{OrderSavedView.count} #{OrderSavedView.new(name: "e", saved_search: s).valid?}"
    out << "base: #{SavedView.respond_to?(:named)} #{SavedView.new.respond_to?(:saved_search)} #{OtherSavedView.new.respond_to?(:pins)} #{SavedView.order(:id).first.class.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    "belongs_to: s true t false Saved search must exist nil",
    "has_many: 2 p1,p2 2 0 p2 nil",
    "scope: 1 a,b b a 0 2",
    "dependent: 0 1 true",
    "base: false false false OrderSavedView",
];

/// What native Rails requires, in order.
const LOADS: [&str; 7] = [
    "app/models/application_record",
    "app/models/saved_search",
    "app/models/saved_view",
    "app/models/order_saved_view",
    "app/models/other_saved_view",
    "app/models/pin",
    "app/models/sti_subclass_macros_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/saved_search.rb", SAVED_SEARCH),
    ("app/models/saved_view.rb", SAVED_VIEW),
    ("app/models/order_saved_view.rb", ORDER_SAVED_VIEW),
    ("app/models/other_saved_view.rb", OTHER_SAVED_VIEW),
    ("app/models/pin.rb", PIN),
    ("app/models/sti_subclass_macros_probe.rb", PROBE),
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
fn sti_subclass_macros_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "sti-subclass-macros",
        &FILES,
        &emitted,
        "rh_spec_sti_subclass_macros",
        "StiSubclassMacrosProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
