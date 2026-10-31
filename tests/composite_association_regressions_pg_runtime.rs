//! PostgreSQL: composite association regressions, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE orders (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    name character varying,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);

CREATE TABLE order_lines (
    id bigserial NOT NULL,
    shop_id bigint,
    order_id bigint,
    sku character varying,
    CONSTRAINT order_lines_pkey PRIMARY KEY (id)
);

CREATE TABLE addresses (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    city character varying,
    CONSTRAINT addresses_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_composite
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  has_many :order_lines, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id], inverse_of: :order
end
"#;

const ORDER_LINE: &str = r#"class OrderLine < ApplicationRecord
  belongs_to :order, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id], inverse_of: :order_lines
end
"#;

const ADDRESS: &str = r#"class Address < ApplicationRecord
  query_constraints :shop_id, :id
end
"#;

const PROBE: &str = r##"class CompositeAssociationRegressionsProbe
  # One answer per line; a step that raises answers its error class, so a
  # failure names the paths it breaks rather than the first of them.
  def self.step(out, label)
    out << "#{label}: #{yield}"
  rescue StandardError, NotImplementedError => e
    out << "#{label}: #{e.class}"
  end

  def self.lines
    out = []
    o1 = Order.create!(shop_id: 1, name: "one")
    o2 = Order.create!(shop_id: 2, name: "two")
    OrderLine.create!(shop_id: 1, order_id: o1.id, sku: "a")
    OrderLine.create!(shop_id: 2, order_id: o2.id, sku: "b")
    OrderLine.create!(shop_id: 2, order_id: o1.id, sku: "x")

    line = OrderLine.new(sku: "w")
    step(out, "writer") { line.order = o1; [line.shop_id, line.order_id == o1.id].inspect }
    step(out, "writer nil") { line.order = nil; [line.shop_id, line.order_id].inspect }
    step(out, "writer saved") do
      line.order = o2
      line.save!
      OrderLine.find(line.id).then { |l| [l.shop_id, l.order_id == o2.id, l.order&.name] }.inspect
    end
    step(out, "partial keys") do
      partial = OrderLine.new(order_id: o1.id)
      first = partial.order
      partial.shop_id = 1
      "#{first.inspect} #{partial.order&.name.inspect}"
    end
    step(out, "partial select") do
      # Rails' `replace_keys` reads each key part with `_read_attribute`: nil
      # for a column the target was loaded without, not a raise.
      partial = OrderLine.new(sku: "p")
      partial.order = Order.select(:id).find(o1.id)
      [partial.shop_id, partial.order_id == o1.id].inspect
    end
    step(out, "where list") { OrderLine.where(order: [o1, o2]).pluck(:sku).sort.inspect }
    step(out, "where list one") { OrderLine.where(order: [o1]).pluck(:sku).sort.inspect }
    step(out, "includes") { OrderLine.includes(:order).order(:id).map { |l| l.order&.name }.inspect }
    step(out, "includes loaded") { OrderLine.includes(:order).order(:id).map { |l| l.association(:order).loaded? }.inspect }
    step(out, "eager_load") { OrderLine.eager_load(:order).where(orders: { name: "one" }).pluck(:sku).sort.inspect }
    step(out, "inverse") do
      owner = Order.find(o1.id)
      built = owner.order_lines.build(sku: "n")
      "#{built.order.equal?(owner)} #{built.shop_id}"
    end
    step(out, "query_foreign_key") do
      [OrderLine.reflect_on_association(:order), Order.reflect_on_association(:order_lines)]
        .map { |r| Array(r.query_foreign_key).map(&:to_s).sort }.inspect
    end
    step(out, "key update") do
      a = Address.create!(shop_id: 1, city: "Oslo")
      a.update!(shop_id: 3, city: "Bergen")
      Address.where(id: a.id).pluck(:shop_id, :city).inspect
    end
    out
  end
end
"##;

const EXPECTED: [&str; 13] = [
    "writer: [1, true]",
    "writer nil: [nil, nil]",
    "writer saved: [2, true, \"two\"]",
    "partial keys: nil \"one\"",
    "partial select: [nil, true]",
    "where list: [\"a\", \"b\", \"w\"]",
    "where list one: [\"a\"]",
    "includes: [\"one\", \"two\", nil, \"two\"]",
    "includes loaded: [true, true, true, true]",
    "eager_load: [\"a\"]",
    "inverse: true 1",
    "query_foreign_key: [[\"order_id\", \"shop_id\"], [\"id\", \"shop_id\"]]",
    "key update: [[3, \"Bergen\"]]",
];

/// What native Rails requires, in order.
const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/order",
    "app/models/order_line",
    "app/models/address",
    "app/models/composite_association_regressions_probe",
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/order.rb", ORDER),
    ("app/models/order_line.rb", ORDER_LINE),
    ("app/models/address.rb", ADDRESS),
    ("app/models/composite_association_regressions_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported"))
        .collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn composite_association_paths_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "composite-association-regressions",
        &FILES,
        &emitted,
        "rh_spec_composite",
        "CompositeAssociationRegressionsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// The emitted model of `file` (`app/models/<file>.rb`).
fn emitted_model(file: &str) -> String {
    let (emitted, _) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    std::fs::read_to_string(emitted.join(format!("app/models/{file}.rb"))).unwrap_or_else(|e| panic!("{file}: {e}"))
}

/// The text of `def <signature>` up to the next method.
fn method_with<'a>(out: &'a str, signature: &str) -> &'a str {
    let start = out.find(&format!("def {signature}")).unwrap_or_else(|| panic!("no def {signature}:\n{out}"));
    let rest = &out[start..];
    let end = ["\n  def ", "\n    def "].iter().filter_map(|next| rest[1..].find(next)).min().map_or(rest.len(), |i| i + 1);
    &rest[..end]
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_composite_belongs_to_writer_stores_every_key_part() {
    let out = emitted_model("order_line");
    // `line.order = order` writes `shop_id` as well (Rails' `replace_keys`),
    // and a nil target clears it.
    let writer = method_with(&out, "order=(value)");
    assert!(writer.contains("@shop_id = nil"), "{writer}");
    assert!(writer.contains("@order_id = value.id_value") || writer.contains("@order_id = value.id"), "{writer}");
    assert!(writer.contains("@shop_id = value._read_attribute(:shop_id)"), "{writer}");
    // A new target is saved first and both parts follow its saved row.
    let autosave = method_with(&out, "_autosave_order");
    assert!(autosave.contains("@shop_id = @order_cache.shop_id"), "{autosave}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_composite_key_part_write_drops_the_loaded_belongs_to_target() {
    let out = emitted_model("order_line");
    // `OrderLine.new(order_id: 1)` reads the association (nil: the shop is
    // missing) before `shop_id=` arrives; Rails' stale_state reads every part.
    let writer = method_with(&out, "shop_id=(value)");
    assert!(writer.contains("@order_loaded = false"), "{writer}");
}

#[test]
fn a_composite_belongs_to_takes_a_preloaded_target() {
    let out = emitted_model("order_line");
    // `includes(:order)` / `eager_load(:order)` seat the batch result
    // through `_preload_order`; the composite reader still queries lazily.
    assert!(out.contains("def _preload_order(rec)"), "{out}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn an_update_addresses_the_row_by_its_stored_query_constraints() {
    let out = emitted_model("address");
    // `address.update!(shop_id: other)` finds the row under the old
    // `shop_id` (Rails' `_query_constraints_hash`).
    assert!(out.contains("attribute_in_database(\"shop_id\")"), "{out}");
}

#[test]
fn a_named_inverse_of_seats_the_owner_on_a_built_record() {
    let out = emitted_model("order");
    assert!(out.contains("record.order = self"), "{out}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_composite_belongs_to_preload_dispatch_marks_the_association_loaded() {
    let out = emitted_model("order_line");
    assert!(out.contains("when :order"), "{out}");
    assert!(out.contains("found = r.order"), "{out}");
}
