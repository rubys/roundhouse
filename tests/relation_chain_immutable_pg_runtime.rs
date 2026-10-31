//! PostgreSQL: relation chain immutable, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE widgets (
    id bigserial NOT NULL,
    type character varying,
    sku character varying,
    kind character varying,
    qty integer,
    active boolean,
    deleted boolean,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_chain_immutable
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  default_scope { where(deleted: false) }

  scope :big, -> { where("qty > ?", 5) }
  scope :by_sku, ->(sku) { where(sku: sku) }
  scope :newest_first, -> { order(id: :desc) }
end
"#;

const GIZMO: &str = r#"class Gizmo < Widget
end
"#;

const PROBE: &str = r##"class ChainImmutableProbe
  def self.skus(rel)
    rel.to_a.map(&:sku).join(",")
  end

  def self.lines
    out = []
    Widget.create!(sku: "a", kind: "x", qty: 1, active: true, deleted: false)
    Widget.create!(sku: "b", kind: "x", qty: 9, active: true, deleted: false)
    Widget.create!(sku: "c", kind: "y", qty: 7, active: true, deleted: false)
    Widget.create!(sku: "d", kind: "y", qty: 3, active: false, deleted: false)
    Widget.create!(sku: "e", kind: "y", qty: 8, active: true, deleted: true)
    Gizmo.create!(sku: "g", kind: "x", qty: 6, active: true, deleted: false)

    base = Widget.where(active: true)
    x = base.where(kind: "x")
    y = base.where(kind: "y")
    out << "base reuse: #{skus(base)} | #{skus(x)} | #{skus(y)} | #{skus(base)}"
    out << "counts: #{base.count} #{x.count} #{y.count} #{base.where(kind: "z").count} #{base.count}"

    ordered = base.order(:sku)
    desc = base.order(sku: :desc)
    out << "order: #{skus(ordered)} | #{skus(desc)} | #{skus(base.reorder(qty: :asc))} | #{skus(ordered)}"
    out << "reorder: #{skus(ordered.reorder(sku: :desc))} | #{skus(ordered)}"

    page = ordered.limit(2)
    out << "limit: #{skus(page)} | #{skus(ordered.limit(1))} | #{skus(page)} | #{skus(ordered)}"
    out << "offset: #{skus(page.offset(1))} | #{skus(page)} | #{skus(ordered.offset(3))} | #{skus(ordered)}"
    out << "limit_value: #{ordered.limit_value.inspect} #{page.limit_value.inspect} #{page.offset(1).offset_value.inspect} #{page.offset_value.inspect}"

    out << "scopes: #{skus(base.big)} | #{skus(base.by_sku("a"))} | #{skus(base.by_sku("c"))} | #{skus(base.big.by_sku("b"))} | #{skus(base)}"
    big = base.big
    out << "scope reuse: #{skus(big.by_sku("c"))} | #{skus(big.newest_first)} | #{skus(big)}"
    out << "scope on model: #{skus(Widget.big.newest_first)} | #{skus(Widget.big)} | #{skus(Widget.by_sku("a").big)}"

    left = Widget.where(kind: "x")
    right = Widget.big
    out << "merge: #{skus(left.merge(right))} | #{skus(left)} | #{skus(right)} | #{skus(right.merge(left))}"
    out << "or: #{skus(left.or(right))} | #{skus(left)} | #{skus(right)}"
    out << "and: #{skus(left.and(right))} | #{skus(left)} | #{skus(right)}"
    out << "not: #{skus(base.where.not(kind: "x"))} | #{skus(base)}"
    out << "excluding: #{skus(base.excluding(Widget.find_by(sku: "a")))} | #{skus(base)}"

    none = base.none
    out << "none: #{none.to_a.size} #{base.count} #{skus(base)}"
    out << "unscope: #{skus(x.unscope(where: :kind))} | #{skus(x)} | #{skus(ordered.unscope(:order))}"
    out << "distinct: #{base.distinct.count} #{base.select(:kind).distinct.order(:kind).map(&:kind).join(",")} | #{skus(base)}"
    out << "select: #{base.select(:sku).map(&:sku).sort.join(",")} | #{skus(base)}"
    out << "group: #{base.group(:kind).count.sort.inspect} | #{base.count}"
    out << "having: #{base.group(:kind).having("COUNT(*) > 2").count.sort.inspect} | #{base.group(:kind).count.sort.inspect}"
    out << "joins: #{skus(base.joins("INNER JOIN widgets other ON other.id = widgets.id"))} | #{skus(base)}"
    out << "readonly: #{base.readonly.first.readonly?} #{base.first.readonly?}"
    out << "lock: #{base.lock.to_sql.include?("FOR UPDATE")} #{base.to_sql.include?("FOR UPDATE")}"

    loaded = base.order(:sku)
    first_pass = skus(loaded)
    narrowed = loaded.where(kind: "y")
    out << "loaded: #{first_pass} | #{skus(narrowed)} | #{skus(loaded)} | #{loaded.to_a.size}"
    out << "loaded limit: #{skus(loaded.limit(1))} | #{loaded.size} | #{skus(loaded.order(sku: :desc))} | #{skus(loaded)}"

    sti = Gizmo.where(active: true)
    out << "sti: #{skus(sti)} | #{skus(sti.where(kind: "y"))} | #{skus(sti.big)} | #{skus(sti)} | #{skus(Gizmo.all)}"
    out << "default scope: #{skus(Widget.all)} | #{skus(Widget.unscoped.where(sku: "e"))} | #{skus(Widget.where(sku: "e"))} | #{skus(Widget.all)}"

    reuse = []
    3.times { |i| reuse << skus(base.where(qty: [1, 9, 7][i])) }
    out << "loop reuse: #{reuse.join(" ")} | #{skus(base)}"
    out << "find_by: #{base.find_by(sku: "a").inspect.include?("Widget")} #{base.find_by(sku: "zz").inspect} #{skus(base)} #{base.count}"
    out << "exists/first/last: #{base.exists?(sku: "a")} #{base.first.sku} #{base.last.sku} #{base.order(:sku).last.sku} #{skus(base)}"
    out << "pluck: #{base.order(:sku).pluck(:sku).join(",")} #{base.order(:sku).limit(2).pluck(:sku).join(",")} #{base.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 32] = [
    "base reuse: a,b,c,g | a,b,g | c | a,b,c,g",
    "counts: 4 3 1 0 4",
    "order: a,b,c,g | g,c,b,a | a,g,c,b | a,b,c,g",
    "reorder: g,c,b,a | a,b,c,g",
    "limit: a,b | a | a,b | a,b,c,g",
    "offset: b,c | a,b | g | a,b,c,g",
    "limit_value: nil 2 1 nil",
    "scopes: b,c,g | a | c | b | a,b,c,g",
    "scope reuse: c | g,c,b | b,c,g",
    "scope on model: g,c,b | b,c,g | ",
    "merge: b,g | a,b,g | b,c,g | b,g",
    "or: a,b,c,g | a,b,g | b,c,g",
    "and: b,g | a,b,g | b,c,g",
    "not: c | a,b,c,g",
    "excluding: b,c,g | a,b,c,g",
    "none: 0 4 a,b,c,g",
    "unscope: a,b,c,g | a,b,g | a,b,c,g",
    "distinct: 4 x,y | a,b,c,g",
    "select: a,b,c,g | a,b,c,g",
    r#"group: [["x", 3], ["y", 1]] | 4"#,
    r#"having: [["x", 3]] | [["x", 3], ["y", 1]]"#,
    "joins: a,b,c,g | a,b,c,g",
    "readonly: true false",
    "lock: true false",
    "loaded: a,b,c,g | c | a,b,c,g | 4",
    "loaded limit: a | 4 | a,b,c,g | a,b,c,g",
    "sti: g |  | g | g | g",
    "default scope: a,b,c,d,g | e |  | a,b,c,d,g",
    "loop reuse: a b c | a,b,c,g",
    "find_by: true nil a,b,c,g 4",
    "exists/first/last: true a g g a,b,c,g",
    "pluck: a,b,c,g a,b 4",
];

/// What native Rails requires, in order.
const LOADS: [&str; 4] = [
    "app/models/application_record",
    "app/models/widget",
    "app/models/gizmo",
    "app/models/chain_immutable_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
    ("app/models/gizmo.rb", GIZMO),
    ("app/models/chain_immutable_probe.rb", PROBE),
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
fn relation_chain_methods_leave_the_receiver_alone_like_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "relation_chain_immutable",
        &FILES,
        &emitted,
        "rh_spec_chain_immutable",
        "ChainImmutableProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
