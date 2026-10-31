//! PostgreSQL: composite primary key, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE orders (
    shop_id bigint NOT NULL,
    id bigint NOT NULL,
    island integer DEFAULT 0 NOT NULL,
    name character varying,
    CONSTRAINT orders_pkey PRIMARY KEY (shop_id, id, island)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_composite_primary_key
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
end
"#;

const PROBE: &str = r##"class CompositePrimaryKeyProbe
  def self.lines
    out = []
    out << "key: #{Order.primary_key.inspect} #{Order.composite_primary_key?} #{Order.query_constraints_list.inspect}"
    a = Order.create!(id: [1, 10, 0], name: "a")
    b = Order.create!(shop_id: 2, id_value: 10, name: "b")
    out << "id: #{a.id.inspect} #{a.id_value.inspect} #{a[:id].inspect} #{a.to_param.inspect} #{a.attributes["id"].inspect} #{a.cache_key.inspect}"
    out << "find: #{Order.find([2, 10, 0]).name} #{Order.find([[1, 10, 0], [2, 10, 0]]).map(&:name).inspect}"
    out << "column: #{Order.where(id: 10).count} #{Order.find_by(id: 10, shop_id: 2).name} #{Order.ids.inspect} #{Order.order(:shop_id).pluck(:id).inspect}"
    a.name = "a2"
    a.save!
    out << "update: #{Order.where(shop_id: 1).pluck(:name).inspect} #{Order.where(shop_id: 2).pluck(:name).inspect}"
    Order.where(shop_id: 2).update_all(name: "b2")
    out << "reload: #{b.reload.name} #{a.reload.name}"
    out << "first: #{Order.first.name} #{Order.last.name}"
    c = Order.new(name: "c")
    c.id = [3, 11, 1]
    c.save!
    out << "assign: #{c.shop_id} #{c.id_value} #{c.island} #{Order.find([3, 11, 1]).name}"
    b.destroy
    out << "destroy: #{Order.count} #{Order.order(:shop_id).pluck(:name).inspect}"
    d = a.dup
    out << "dup: #{d.new_record?} #{d.shop_id.inspect} #{d.id_value.inspect} #{d.island.inspect} #{d.name}"
    out
  end
end
"##;

const EXPECTED: [&str; 10] = [
    r#"key: ["shop_id", "id", "island"] true ["shop_id", "id", "island"]"#,
    r#"id: [1, 10, 0] 10 10 "1_10_0" 10 "orders/[1, 10, 0]""#,
    r#"find: b ["a", "b"]"#,
    "column: 2 b [[1, 10, 0], [2, 10, 0]] [10, 10]",
    r#"update: ["a2"] ["b"]"#,
    "reload: b2 a2",
    "first: a2 b2",
    "assign: 3 11 1 c",
    r#"destroy: 2 ["a2", "c"]"#,
    "dup: true nil nil nil a2",
];

/// What native Rails requires, in order.
const LOADS: [&str; 3] = ["app/models/application_record", "app/models/order", "app/models/composite_primary_key_probe"];

const FILES: [(&str, &str); 7] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/order.rb", ORDER),
    ("app/models/composite_primary_key_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The compiler side: `id` is the tuple over the key columns, the `id`
/// column `id_value`, and the class answers the column list.
#[test]
#[ignore = "fork spec: postgresql"]
fn composite_primary_key_compiles_to_the_tuple() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let order = std::fs::read_to_string(emitted.join("app/models/order.rb")).unwrap();
    for expected in [
        // `id` in the model's GeneratedAttributeMethods module.
        "    def id\n      [self.shop_id, self.id_value, self.island]\n    end\n",
        "  def id_value\n    @id\n  end\n",
        "  def self.primary_key\n    [\"shop_id\", \"id\", \"island\"]\n  end\n",
        // `shop_id` and `id` are NOT NULL with no default, so they read nil when unset and bind
        // through the `_opt` primitive (an unset one reaches the database as NULL, as in Rails);
        // `island` has a default.
        r#"WHERE \"shop_id\" = " + Db.escape_int_opt(@shop_id) + " AND \"id\" = " + Db.escape_int_opt(@id) + " AND \"island\" = "#,
    ] {
        assert!(order.contains(expected), "missing {expected:?} in:\n{order}");
    }
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn composite_primary_key_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "composite-primary-key",
        &FILES,
        &emitted,
        "rh_spec_composite_primary_key",
        "CompositePrimaryKeyProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
