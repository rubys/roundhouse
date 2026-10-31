//! PostgreSQL: where column hash partial select, on the Ruby target against native Rails.
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
    shop_id bigint NOT NULL,
    kind character varying NOT NULL,
    quantity integer NOT NULL,
    context character varying(255),
    created_at timestamp(6) without time zone NOT NULL,
    updated_at timestamp(6) without time zone NOT NULL,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_where_column_hash
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SHOP_TYPE: &str = r#"class ShopType < ActiveRecord::Type::BigInteger
  def deserialize(value)
    shop_id = super
    raise "Widget from shop #{shop_id} was accessed from shop 7" if !shop_id.nil? && shop_id != 7
    shop_id
  end
end
"#;

const CONTEXT_TYPE: &str = r#"class ContextType < ActiveRecord::Type::Value
  def type
    :context
  end

  def serialize(value)
    value.nil? ? nil : JSON.generate(value)
  end

  def deserialize(value)
    value.nil? ? nil : JSON.parse(value)
  end
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  attribute :shop_id, ShopType.new
  attribute :context, ContextType.new
end
"#;

const PROBE: &str = r##"class WhereColumnHashProbe
  def self.lines
    out = []
    now = Time.now
    Widget.create!(shop_id: 7, kind: "a", quantity: 1, context: { "region" => "ca" }, created_at: now, updated_at: now)
    Widget.create!(shop_id: 7, kind: "b", quantity: 2, context: { "region" => "us" }, created_at: now, updated_at: now)
    Widget.create!(shop_id: 7, kind: "a", quantity: 3, created_at: now, updated_at: now)
    kinds = ["widgets.id", "widgets.kind"]
    out << "group: #{Widget.group(kinds).order(:id).map(&:id).inspect}"
    out << "group_count: #{Widget.group(["widgets.kind"]).having("COUNT(*) > 0").count.sort.inspect}"
    out << "context: #{Widget.where(context: { "region" => "ca" }).map(&:quantity).inspect}"
    out << "context_with_shop: #{Widget.where(shop_id: 7, context: { "region" => "us" }).count}"
    out << "partial: #{Widget.order(:id).select(:id, :quantity).map { |w| [w.id.class, w.quantity] }.inspect}"
    out << "unloaded: #{Widget.select(:id, :quantity).first.has_attribute?(:shop_id)}"
    out << "full: #{Widget.order(:id).map(&:shop_id).inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 7] = [
    "group: [1, 2, 3]",
    "group_count: [[\"a\", 2], [\"b\", 1]]",
    "context: [1]",
    "context_with_shop: 1",
    "partial: [[Integer, 1], [Integer, 2], [Integer, 3]]",
    "unloaded: false",
    "full: [7, 7, 7]",
];

const LOADS: [&str; 5] = [
    "app/models/application_record",
    "app/models/shop_type",
    "app/models/context_type",
    "app/models/widget",
    "app/models/where_column_hash_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/shop_type.rb", SHOP_TYPE),
    ("app/models/context_type.rb", CONTEXT_TYPE),
    ("app/models/widget.rb", WIDGET),
];

fn files() -> Vec<(&'static str, &'static str)> {
    let mut all = FILES.to_vec();
    all.push(("app/models/where_column_hash_probe.rb", PROBE));
    all
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn column_hash_where_and_partial_select_match_native_rails_on_postgresql() {
    let files = files();
    let overlay = files.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "where-column-hash",
        &files,
        &emitted,
        "rh_spec_where_column_hash",
        "WhereColumnHashProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
