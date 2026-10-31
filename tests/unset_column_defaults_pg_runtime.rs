//! PostgreSQL: unset column defaults, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;

const STRUCTURE: &str = r#"CREATE TABLE items (
    id bigserial NOT NULL,
    label character varying NOT NULL,
    body text NOT NULL,
    note character varying DEFAULT ''::character varying NOT NULL,
    kind character varying(32) DEFAULT 'plain'::character varying NOT NULL,
    nickname character varying,
    gift_card boolean DEFAULT false,
    featured boolean DEFAULT true NOT NULL,
    qty integer DEFAULT 5 NOT NULL,
    owner_type_int integer NOT NULL,
    weight bigint NOT NULL,
    batch_id bigint NOT NULL DEFAULT 0,
    delta integer DEFAULT -1,
    price numeric(21,3) DEFAULT 0.0,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  encoding: unicode\n  database: rh_spec_unset_defaults\n";
const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const ITEM: &str = r##"class Item < ApplicationRecord
  validates :label, presence: true
  validates :gift_card, inclusion: { in: [true, false] }

  before_validation :assign_reference, on: :create

  def reference
    @reference
  end

  private

  def assign_reference
    @reference ||= label
  end
end
"##;

const PROBE: &str = r##"class DefaultsProbe
  def self.lines
    item = Item.new
    out = []
    out << "label: #{item.label.inspect}"
    out << "body: #{item.body.inspect}"
    out << "note: #{item.note.inspect}"
    out << "kind: #{item.kind.inspect}"
    out << "nickname: #{item.nickname.inspect}"
    out << "gift_card: #{item.gift_card.inspect}"
    out << "featured: #{item.featured.inspect}"
    out << "qty: #{item.qty.inspect}"
    out << "owner_type_int: #{item.owner_type_int.inspect}"
    out << "weight: #{item.weight.inspect}"
    out << "batch_id: #{item.batch_id.inspect}"
    out << "owner_type_int fallback: #{(item.owner_type_int || 7).inspect}"
    out << "delta: #{item.delta.inspect}"
    out << "price: #{item.price.to_s}"
    out << "valid: #{item.valid?} #{item.errors.full_messages.inspect}"
    given = Item.new(label: "tag", qty: 2, gift_card: true, owner_type_int: 3)
    out << "given: #{given.label.inspect} #{given.qty.inspect} #{given.gift_card.inspect} #{given.owner_type_int.inspect} #{given.valid?}"
    saved = Item.create!(label: "kept", body: "text", owner_type_int: 4, weight: 9)
    loaded = Item.find(saved.id)
    out << "loaded: #{loaded.label.inspect} #{loaded.gift_card.inspect} #{loaded.qty.inspect} #{loaded.delta.inspect} #{loaded.owner_type_int.inspect} #{loaded.weight.inspect} #{loaded.batch_id.inspect}"
    refused = begin
      Item.create!(label: "x", body: "y")
      "saved"
    rescue ActiveRecord::NotNullViolation
      "NotNullViolation"
    end
    out << "refused: #{refused}"
    out
  end
end
"##;

const EXPECTED: &[&str] = &[
    "label: nil",
    "body: nil",
    "note: \"\"",
    "kind: \"plain\"",
    "nickname: nil",
    "gift_card: false",
    "featured: true",
    "qty: 5",
    "owner_type_int: nil",
    "weight: nil",
    "batch_id: 0",
    "owner_type_int fallback: 7",
    "delta: -1",
    "price: 0.0",
    "valid: false [\"Label can't be blank\"]",
    "given: \"tag\" 2 true 3 true",
    "loaded: \"kept\" false 5 -1 4 9 0",
    "refused: NotNullViolation",
];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item.rb", ITEM),
    ("app/models/defaults_probe.rb", PROBE),
];

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/item", "app/models/defaults_probe"];

#[test]
#[ignore = "fork spec: postgresql"]
fn unset_columns_read_like_rails_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "unset-column-defaults",
        &FILES,
        &emitted,
        "rh_spec_unset_defaults",
        "DefaultsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
