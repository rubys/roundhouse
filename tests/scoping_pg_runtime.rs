//! PostgreSQL: scoping, on the Ruby target against native Rails.
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
    island integer,
    name character varying,
    live boolean,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);

CREATE TABLE notes (
    id bigserial NOT NULL,
    type character varying,
    title character varying,
    status character varying,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_scoping
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP_RECORD: &str = r#"class ShopRecord < ApplicationRecord
  self.abstract_class = true

  def self.unscoped(&block)
    relation = super(&nil)
    island_scope = if current_scope
      current_scope.where_values_hash.slice("island")
    else
      { island: 1 }
    end
    relation = relation.where(island_scope)
    if block
      relation.scoping(&block)
    else
      relation
    end
  end
end
"#;

const ITEM: &str = r#"class Item < ShopRecord
  default_scope { where(live: true) }
end
"#;

const NOTE: &str = r#"class Note < ApplicationRecord
end
"#;

const PINNED: &str = r#"class Pinned < Note
end
"#;

const PROBE: &str = r##"class ScopingProbe
  def self.names(items)
    items.map(&:name).sort.join(",")
  end

  def self.lines
    out = []
    Item.unscoped.create!(island: 1, name: "a", live: true)
    Item.unscoped.create!(island: 1, name: "b", live: false)
    Item.unscoped.create!(island: 2, name: "c", live: true)
    Item.unscoped.create!(island: 2, name: "d", live: false)
    out << "outside: #{Item.current_scope.inspect} #{Item.count} #{names(Item.unscoped)} #{names(Item.all)}"
    out << "where_values_hash: #{Item.unscoped.where(name: "a", island: 3).where_values_hash.sort.inspect} #{Item.where(name: nil).where_values_hash.sort.inspect}"
    out << "scope_for_create: #{Item.where(name: "a").scope_for_create.sort.inspect} #{Item.unscoped.where(island: 3, name: %w[a b]).scope_for_create.sort.inspect}"
    out << Item.where(island: 2).scoping {
      "scoping: #{Item.current_scope.where_values_hash.sort.inspect} #{names(Item.all)} #{Item.count} #{names(Item.unscoped)} #{Item.where(name: "c").count} #{Item.find_by(name: "d").inspect}"
    }
    out << Item.unscoped {
      "unscoped block: #{Item.current_scope.where_values_hash.sort.inspect} #{Item.count} #{names(Item.all)} #{names(Item.where(live: false))}"
    }
    out << Item.where(island: 2).scoping {
      inner = Item.unscoped { "#{Item.count} #{names(Item.all)}" }
      "nested: #{inner} #{Item.count} #{Item.current_scope.where_values_hash.sort.inspect}"
    }
    out << "after: #{Item.current_scope.inspect} #{Item.count}"
    caught = begin
      Item.where(island: 2).scoping { raise ArgumentError, "x" }
    rescue ArgumentError
      Item.current_scope.inspect
    end
    out << "raised: #{caught}"
    Note.create!(title: "n", status: "live")
    Pinned.create!(title: "p", status: "live")
    Pinned.create!(title: "q", status: "draft")
    out << Note.where(status: "live").scoping {
      "sti: #{Pinned.current_scope.nil?} #{Pinned.current_scope(true).nil?} #{Note.count} #{Pinned.count} #{Pinned.all.map(&:title).sort.join(",")}"
    }
    out << "descends: #{ActiveRecord::Base.descends_from_active_record?} #{ApplicationRecord.descends_from_active_record?} #{Item.descends_from_active_record?} #{Note.descends_from_active_record?} #{Pinned.descends_from_active_record?} #{Pinned.sti_name}"
    out
  end
end
"##;

const EXPECTED: [&str; 10] = [
    "outside: nil 2 a,b a,c",
    r#"where_values_hash: [["island", 3], ["name", "a"]] [["live", true], ["name", nil]]"#,
    r#"scope_for_create: [["live", true], ["name", "a"]] [["island", 3]]"#,
    r#"scoping: [["island", 2], ["live", true]] c 1 c,d 1 nil"#,
    r#"unscoped block: [["island", 1]] 2 a,b b"#,
    r#"nested: 2 c,d 1 [["island", 2], ["live", true]]"#,
    "after: nil 2",
    "raised: nil",
    "sti: false true 2 1 p",
    "descends: false true true true false Pinned",
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/shop_record",
    "app/models/item",
    "app/models/note",
    "app/models/pinned",
    "app/models/scoping_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop_record.rb", SHOP_RECORD),
    ("app/models/item.rb", ITEM),
    ("app/models/note.rb", NOTE),
    ("app/models/pinned.rb", PINNED),
    ("app/models/scoping_probe.rb", PROBE),
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
fn scoping_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("scoping", &FILES, &emitted, "rh_spec_scoping", "ScopingProbe", &LOADS) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
