//! PostgreSQL: relation find nth, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE items (
    id bigserial NOT NULL,
    name character varying,
    rank integer,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_find_nth
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ITEM: &str = "class Item < ApplicationRecord\nend\n";

const PROBE: &str = r##"class FindNthProbe
  def self.names(&block)
    block.call
  rescue ActiveRecord::RecordNotFound
    "not found"
  end

  def self.lines
    out = []
    %w[a b c d e].each_with_index { |n, i| Item.create!(name: n, rank: 10 - i) }
    items = Item.all
    out << "ordinals: #{[items.second, items.third, items.fourth, items.fifth].map(&:name).inspect}"
    out << "forty_two: #{items.forty_two.inspect}"
    out << "bang: #{[items.second!, items.fifth!].map(&:name).inspect}"
    out << "bang miss: #{names { items.forty_two! }}"
    out << "to_last: #{[items.second_to_last, items.third_to_last].map(&:name).inspect} miss=#{Item.where(name: "a").second_to_last.inspect}"
    out << "order: #{Item.order(:rank).second.name} #{Item.order(rank: :desc).third.name}"
    out << "where: #{Item.where("rank < 8").second.name} #{Item.where(name: "zz").second.inspect}"
    out << "offset: #{Item.offset(1).second.name} #{Item.offset(3).third.inspect}"
    out << "limit: #{Item.limit(3).third.name} #{Item.limit(3).fourth.inspect} #{Item.limit(2).second.name}"
    loaded = Item.order(:rank).load
    out << "loaded: #{loaded.second.name} #{loaded.fifth.name} #{loaded.forty_two.inspect}"
    out << "after: #{items.second.name} #{items.to_a.length} #{Item.where(name: "a").second.inspect}"
    out
  end
end
"##;

const LOADS: [&str; 3] = ["app/models/application_record", "app/models/item", "app/models/find_nth_probe"];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item.rb", ITEM),
    ("app/models/find_nth_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

const EXPECTED: [&str; 11] = [
    "ordinals: [\"b\", \"c\", \"d\", \"e\"]",
    "forty_two: nil",
    "bang: [\"b\", \"e\"]",
    "bang miss: not found",
    "to_last: [\"d\", \"c\"] miss=nil",
    "order: d c",
    "where: e nil",
    "offset: c nil",
    "limit: c nil b",
    "loaded: d a nil",
    "after: b 5 nil",
];

#[test]
#[ignore = "fork spec: postgresql"]
fn relation_find_nth_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "find-nth",
        &FILES,
        &emitted,
        "rh_spec_find_nth",
        "FindNthProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
