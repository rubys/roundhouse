//! PostgreSQL: model surface, on the Ruby target against native Rails.
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
    kind character varying,
    stamped_at timestamp without time zone,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);

CREATE TABLE parts (
    id bigserial NOT NULL,
    item_id bigint NOT NULL,
    n integer NOT NULL,
    CONSTRAINT parts_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  encoding: unicode\n  database: rh_spec_rt\n";
const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const ITEM: &str = r##"class Item < ApplicationRecord
  has_many :parts
  before_validation :derive_kind

  private

  def derive_kind
    self.kind ||= label.to_s.upcase
  end
end
"##;

const PART: &str = "class Part < ApplicationRecord\n  belongs_to :item\nend\n";

const PROBE: &str = r##"class RuntimeProbe
  def self.lines
    out = []
    item = Item.new(label: "tag")
    out << "valid: #{item.valid?} kind=#{item.kind.inspect}"
    Item.insert_all!([{ "id" => nil, label: "bulk", kind: "k" }.merge(id: 900)], unique_by: :id)
    out << "bulk: #{Item.find(900).label}"
    parent = Item.create!(label: "owner")
    [1, 2, 3, 4].each { |n| parent.parts.create!(n: n) }
    out << "between: #{Part.where(n: 2..3).order(:n).pluck(:n).inspect}"
    out << "exclusive: #{Part.where(n: 2...4).order(:n).pluck(:n).inspect}"
    out << "endless: #{Part.where(n: 3..).order(:n).pluck(:n).inspect}"
    out << "beginless: #{Part.where(n: ..2).order(:n).pluck(:n).inspect}"
    stamp = Time.utc(2020, 1, 1)
    parent.update!(stamped_at: stamp)
    out << "time range: #{Item.where(stamped_at: (stamp - 60)..(stamp + 60)).pluck(:id) == [parent.id]}"
    picked = parent.parts.select(:id, :n).order(:n).to_a
    out << "select: #{picked.map(&:n).inspect}"
    out << "block select: #{parent.parts.select { |p| p.n.even? }.map(&:n).sort.inspect}"
    out
  end
end
"##;

const EXPECTED: &[&str] = &[
    "valid: true kind=\"TAG\"",
    "bulk: bulk",
    "between: [2, 3]",
    "exclusive: [2, 3]",
    "endless: [3, 4]",
    "beginless: [1, 2]",
    "time range: true",
    "select: [1, 2, 3, 4]",
    "block select: [2, 4]",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item.rb", ITEM),
    ("app/models/part.rb", PART),
    ("app/models/runtime_probe.rb", PROBE),
];

const LOADS: [&str; 4] =
    ["app/models/application_record", "app/models/item", "app/models/part", "app/models/runtime_probe"];

#[test]
#[ignore = "fork spec: postgresql native control"]
fn bare_valid_bulk_insert_range_and_association_select_match_rails_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "model_surface-runtime",
        &FILES,
        &emitted,
        "rh_spec_rt",
        "RuntimeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    assert_eq!(emitted_lines, native, "emitted");
}
