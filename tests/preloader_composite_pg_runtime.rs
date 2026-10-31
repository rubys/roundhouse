//! PostgreSQL: preloader composite, on the Ruby target against native Rails.
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
    shop_id bigint NOT NULL,
    order_id bigint,
    sku character varying,
    CONSTRAINT order_lines_pkey PRIMARY KEY (id)
);

CREATE TABLE line_adjustments (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    order_line_id bigint,
    delta integer,
    CONSTRAINT line_adjustments_pkey PRIMARY KEY (id)
);

CREATE TABLE drafts (
    id bigserial NOT NULL,
    shop_id bigint NOT NULL,
    order_id bigint,
    title character varying,
    CONSTRAINT drafts_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_probe_r6_preload_counts
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ORDER: &str = r#"class Order < ApplicationRecord
  has_many :order_lines, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id]
  has_one :draft, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id]
end
"#;

const ORDER_LINE: &str = r#"class OrderLine < ApplicationRecord
  belongs_to :order, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id]
  has_many :line_adjustments, foreign_key: [:shop_id, :order_line_id], primary_key: [:shop_id, :id]
  has_many :id_adjustments, class_name: "LineAdjustment", foreign_key: [:order_line_id, :shop_id], primary_key: [:id, :shop_id]
end
"#;

const LINE_ADJUSTMENT: &str = r#"class LineAdjustment < ApplicationRecord
  belongs_to :order_line, foreign_key: [:shop_id, :order_line_id], primary_key: [:shop_id, :id]
end
"#;

const DRAFT: &str = r#"class Draft < ApplicationRecord
  belongs_to :order, foreign_key: [:shop_id, :order_id], primary_key: [:shop_id, :id]
end
"#;

const PROBE: &str = r##"class PreloaderCompositeProbe
  # The `sql.active_record` event names a block issues, schema lookups aside.
  def self.names
    events = []
    sub = ActiveSupport::Notifications.subscribe("sql.active_record") do |*, payload|
      events << payload[:name] unless payload[:name] == "SCHEMA"
    end
    yield
    events
  ensure
    ActiveSupport::Notifications.unsubscribe(sub)
  end

  def self.orders
    Order.order(:id).to_a
  end

  def self.skus(order)
    order.order_lines.map(&:sku).sort
  end

  def self.lines
    out = []
    o1 = Order.create!(shop_id: 1, name: "one")
    o2 = Order.create!(shop_id: 2, name: "two")
    o3 = Order.create!(shop_id: 1, name: "three")
    Order.create!(shop_id: 3, name: "four")
    l1 = OrderLine.create!(shop_id: 1, order_id: o1.id, sku: "a")
    l2 = OrderLine.create!(shop_id: 1, order_id: o1.id, sku: "b")
    l3 = OrderLine.create!(shop_id: 2, order_id: o2.id, sku: "c")
    # Shares an order id with o2 and a shop with o1, and is nobody's.
    OrderLine.create!(shop_id: 1, order_id: o2.id, sku: "stray")
    LineAdjustment.create!(shop_id: 1, order_line_id: l1.id, delta: 1)
    LineAdjustment.create!(shop_id: 1, order_line_id: l1.id, delta: 2)
    LineAdjustment.create!(shop_id: 2, order_line_id: l3.id, delta: 5)
    Draft.create!(shop_id: 1, order_id: o1.id, title: "d1")
    Draft.create!(shop_id: 2, order_id: o1.id, title: "other shop")
    Draft.create!(shop_id: 1, order_id: o3.id, title: "d3")

    s = orders
    out << "has_many events: #{names { ActiveRecord::Associations::Preloader.new(records: s, associations: :order_lines).call }.inspect}"
    out << "has_many: #{s.map { |o| o.association(:order_lines).loaded? }.inspect} #{s.map { |o| skus(o) }.inspect}"

    s = orders
    out << "has_one events: #{names { ActiveRecord::Associations::Preloader.new(records: s, associations: :draft).call }.inspect}"
    out << "has_one: #{s.map { |o| o.association(:draft).loaded? }.inspect} #{s.map { |o| o.draft&.title }.inspect}"

    s = orders
    out << "nested events: #{names { ActiveRecord::Associations::Preloader.new(records: s, associations: { order_lines: :line_adjustments }).call }.inspect}"
    adj = s.map { |o| o.order_lines.sort_by(&:sku).map { |l| [l.association(:line_adjustments).loaded?, l.line_adjustments.map(&:delta).sort] } }
    out << "nested: #{adj.inspect}"

    s = orders
    out << "id first events: #{names { ActiveRecord::Associations::Preloader.new(records: s, associations: { order_lines: :id_adjustments }).call }.inspect}"
    out << "id first: #{s.map { |o| o.order_lines.sort_by(&:sku).map { |l| [l.association(:id_adjustments).loaded?, l.id_adjustments.map(&:delta).sort] } }.inspect}"

    s = orders
    out << "listed events: #{names { ActiveRecord::Associations::Preloader.new(records: s, associations: [:draft, { order_lines: :line_adjustments }]).call }.inspect}"

    s = orders
    s.first.association(:order_lines).target = [OrderLine.new(sku: "kept")]
    out << "loaded kept events: #{names { ActiveRecord::Associations::Preloader.new(records: s, associations: :order_lines).call }.inspect}"
    out << "loaded kept: #{s.map { |o| skus(o) }.inspect}"

    out << "includes events: #{names { Order.includes(order_lines: :line_adjustments).order(:id).to_a }.inspect}"
    out << "preload events: #{names { Order.preload(:order_lines, :draft).order(:id).to_a }.inspect}"
    out << "lazy events: #{names { orders.each { |o| o.order_lines.to_a } }.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 14] = [
    r#"has_many events: ["OrderLine Load"]"#,
    r#"has_many: [true, true, true, true] [["a", "b"], ["c"], [], []]"#,
    r#"has_one events: ["Draft Load"]"#,
    r#"has_one: [true, true, true, true] ["d1", nil, "d3", nil]"#,
    r#"nested events: ["OrderLine Load", "LineAdjustment Load"]"#,
    r#"nested: [[[true, [1, 2]], [true, []]], [[true, [5]]], [], []]"#,
    r#"id first events: ["OrderLine Load", "LineAdjustment Load"]"#,
    r#"id first: [[[true, [1, 2]], [true, []]], [[true, [5]]], [], []]"#,
    r#"listed events: ["Draft Load", "OrderLine Load", "LineAdjustment Load"]"#,
    r#"loaded kept events: ["OrderLine Load"]"#,
    r#"loaded kept: [["kept"], ["c"], [], []]"#,
    r#"includes events: ["Order Load", "OrderLine Load", "LineAdjustment Load"]"#,
    r#"preload events: ["Order Load", "OrderLine Load", "Draft Load"]"#,
    r#"lazy events: ["Order Load", "OrderLine Load", "OrderLine Load", "OrderLine Load", "OrderLine Load"]"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/order",
    "app/models/order_line",
    "app/models/line_adjustment",
    "app/models/draft",
    "app/models/preloader_composite_probe",
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/order.rb", ORDER),
    ("app/models/order_line.rb", ORDER_LINE),
    ("app/models/line_adjustment.rb", LINE_ADJUSTMENT),
    ("app/models/draft.rb", DRAFT),
    ("app/models/preloader_composite_probe.rb", PROBE),
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
fn composite_preloader_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "preloader_composite",
        &FILES,
        &emitted,
        "rh_spec_probe_r6_preload_counts",
        "PreloaderCompositeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
