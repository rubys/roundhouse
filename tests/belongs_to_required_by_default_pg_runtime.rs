//! PostgreSQL: belongs to required by default, on the Ruby target against native Rails.
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

CREATE TABLE loose_items (
    id bigserial NOT NULL,
    shop_id bigint,
    CONSTRAINT loose_items_pkey PRIMARY KEY (id)
);

CREATE TABLE strict_items (
    id bigserial NOT NULL,
    shop_id bigint,
    CONSTRAINT strict_items_pkey PRIMARY KEY (id)
);
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_belongs_to_required_by_default
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
  self.belongs_to_required_by_default = true
end
"#;

const FIX_ME: &str = r#"module FixMeMyModelHasImplicitAssociationValidation
  extend self

  def extended(base)
    base.belongs_to_required_by_default = false
  end
end
"#;

const SHOP: &str = r#"class Shop < ApplicationRecord
end
"#;

const LOOSE_ITEM: &str = r#"class LooseItem < ApplicationRecord
  extend FixMeMyModelHasImplicitAssociationValidation

  belongs_to :shop
end
"#;

const STRICT_ITEM: &str = r#"class StrictItem < ApplicationRecord
  belongs_to :shop
end
"#;

const PROBE: &str = r##"class RequiredByDefaultProbe
  def self.lines
    out = []
    out << "settings: #{ApplicationRecord.belongs_to_required_by_default} #{LooseItem.belongs_to_required_by_default} #{StrictItem.belongs_to_required_by_default?} #{Shop.belongs_to_required_by_default}"
    loose = LooseItem.new
    out << "loose: #{loose.save} #{loose.persisted?} #{LooseItem.find(loose.id).shop_id.inspect}"
    strict = StrictItem.new
    out << "strict: #{strict.valid?} #{strict.errors.full_messages.inspect} #{strict.save}"
    s = Shop.create!(name: "one")
    out << "strict with shop: #{StrictItem.new(shop: s).save}"
    out
  end
end
"##;

const EXPECTED: [&str; 4] = [
    "settings: true false true true",
    "loose: true true nil",
    "strict: false [\"Shop must exist\"] false",
    "strict with shop: true",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/concerns/fix_me_my_model_has_implicit_association_validation",
    "app/models/application_record",
    "app/models/shop",
    "app/models/loose_item",
    "app/models/strict_item",
    "app/models/required_by_default_probe",
];

const FILES: [(&str, &str); 11] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/concerns/fix_me_my_model_has_implicit_association_validation.rb", FIX_ME),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop.rb", SHOP),
    ("app/models/loose_item.rb", LOOSE_ITEM),
    ("app/models/strict_item.rb", STRICT_ITEM),
    ("app/models/required_by_default_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn belongs_to_required_by_default_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "belongs-to-required-by-default",
        &FILES,
        &emitted,
        "rh_spec_belongs_to_required_by_default",
        "RequiredByDefaultProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let refused: Vec<&String> = errors.iter().filter(|e| e.contains("belongs_to_required_by_default")).collect();
    assert!(refused.is_empty(), "{refused:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}

/// The emitted tree's Base carries the class attribute the `extended`
/// hook writes; without PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn belongs_to_required_by_default_is_carried_by_the_runtime_base() {
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let core = std::fs::read_to_string(emitted.join("runtime/active_record_core.rb")).unwrap();
    for def in [
        "def belongs_to_required_by_default\n",
        "def belongs_to_required_by_default=(value)\n",
        "def belongs_to_required_by_default?\n",
    ] {
        assert!(core.contains(def), "no `{def}` on Base");
    }
    let loose = std::fs::read_to_string(emitted.join("app/models/loose_item.rb")).unwrap();
    assert!(loose.contains("extend FixMeMyModelHasImplicitAssociationValidation"), "{loose}");
}
