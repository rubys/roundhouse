//! PostgreSQL: relation column lists touch dup, on the Ruby target against native Rails.
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
    name character varying,
    number integer,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    touched_at timestamp(6) without time zone,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_column_lists_touch_dup
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  COLUMNS = [:name, :number].freeze

  before_validation :default_name, on: :create

  def default_name
    self.name = "defaulted" if name.nil?
  end
end
"#;

const PROBE: &str = r##"class ColumnListsProbe
  def self.lines
    out = []
    Widget.create!(name: "a", number: 1)
    Widget.create!(name: "b", number: 2)
    scope = Widget.order(:id)
    out << "select const: #{scope.select(Widget::COLUMNS).map { |w| [w.name, w.number] }.inspect}"
    out << "select array: #{scope.select([:name]).map(&:name).inspect}"
    out << "pluck array: #{scope.pluck([:name, :number]).inspect}"
    out << "pluck const: #{scope.pluck(Widget::COLUMNS).inspect} #{scope.pluck([:name]).inspect}"
    out << "pick array: #{scope.pick([:name, :number]).inspect}"

    w = Widget.find_by!(name: "a")
    before = w.updated_at
    # `send` reaches the runtime `touch(*names)` (what `super` in Product#touch
    # does); a literal `w.touch(:col)` is lowered at the call site.
    out << "touch names: #{w.send(:touch, :touched_at)} #{w.touched_at.nil?} #{w.touched_at == w.updated_at} #{w.updated_at >= before}"
    out << "touch persisted: #{Widget.find(w.id).touched_at.nil?} #{w.changed?}"
    t = Time.utc(2020, 1, 2, 3, 4, 5)
    w.touch(:touched_at, time: t)
    fresh = Widget.find(w.id)
    out << "touch time: #{fresh.touched_at.utc.to_i == t.to_i} #{fresh.updated_at.utc.to_i == t.to_i}"
    out << "touch bare: #{w.touch} #{Widget.find(w.id).updated_at.utc.to_i != t.to_i}"

    d = w.dup
    out << "dup: #{d.new_record?} #{d.persisted?} #{d.id.nil? || d.id == 0} #{d.name} #{d.created_at.inspect} #{d.updated_at.inspect}"
    d.name = "copy"
    d.save!
    out << "dup saved: #{d.persisted?} #{d.id != w.id} #{Widget.count} #{Widget.find(w.id).name} #{Widget.find(d.id).name}"
    out
  end
end
"##;

const EXPECTED: [&str; 11] = [
    r#"select const: [["a", 1], ["b", 2]]"#,
    r#"select array: ["a", "b"]"#,
    r#"pluck array: [["a", 1], ["b", 2]]"#,
    r#"pluck const: [["a", 1], ["b", 2]] ["a", "b"]"#,
    r#"pick array: ["a", 1]"#,
    r#"touch names: true false true true"#,
    r#"touch persisted: false false"#,
    r#"touch time: true true"#,
    r#"touch bare: true true"#,
    r#"dup: true false true a nil nil"#,
    r#"dup saved: true true 3 a copy"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 3] = ["app/models/application_record", "app/models/widget", "app/models/column_lists_probe"];

const FILES: [(&str, &str); 6] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/widget.rb", WIDGET),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
        .write("app/models/column_lists_probe.rb", PROBE)
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn column_lists_touch_names_and_dup_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let mut files: Vec<(&str, &str)> = FILES.to_vec();
    files.push(("app/models/column_lists_probe.rb", PROBE));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "column-lists-touch-dup",
        &files,
        &emitted,
        "rh_spec_column_lists_touch_dup",
        "ColumnListsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
