//! PostgreSQL: new record dirty baseline, on the Ruby target against native Rails.
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
    status character varying DEFAULT 'start'::character varying NOT NULL,
    quantity integer DEFAULT 0 NOT NULL,
    express boolean DEFAULT false NOT NULL,
    note character varying,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    CONSTRAINT orders_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_new_record_baseline
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ORDER: &str = r##"class Order < ApplicationRecord
  before_validation :log_before_validation

  def log_before_validation
    ChangeLog.add("bv[#{changes.inspect}]")
  end
end
"##;

const CHANGE_LOG: &str = r#"class ChangeLog
  def self.entries
    @entries ||= []
  end

  def self.add(entry)
    entries << entry
  end

  def self.take
    taken = entries.join(" ")
    entries.clear
    taken
  end
end
"#;

const PROBE: &str = r##"class NewRecordBaselineProbe
  def self.insert_columns
    columns = []
    ActiveSupport::Notifications.subscribed(->(*, payload) { columns << payload[:sql][/INSERT INTO "orders" \((.*?)\) VALUES/, 1].to_s.delete('"') if payload[:sql].to_s.start_with?("INSERT") }, "sql.active_record") do
      yield
    end
    columns.join(" | ")
  end

  def self.lines
    out = []
    o = Order.new
    out << "bare: changed=#{o.changed.inspect} changes=#{o.changes.inspect} status_was=#{o.status_was.inspect}"
    o = Order.new(status: "open")
    out << "status: status_was=#{o.status_was.inspect} changed=#{o.changed.inspect} changes=#{o.changes.inspect} changed?=#{o.status_changed?} attr_was=#{o.attribute_was(:status).inspect}"
    o = Order.new(status: "start", quantity: 0, express: false)
    out << "same as default: changed=#{o.changed.inspect} has_changes=#{o.has_changes_to_save?}"
    o = Order.new(quantity: 3, note: "n")
    out << "mixed: changed=#{o.changed.sort.inspect} quantity=#{o.changes["quantity"].inspect} note=#{o.changes["note"].inspect} status_was=#{o.status_was.inspect}"
    o.status = "open"
    o.status = "start"
    out << "written back: changed=#{o.changed.sort.inspect}"
    o = Order.new
    o.status = "open"
    out << "writer: status_was=#{o.status_was.inspect} changes=#{o.changes.inspect}"
    ChangeLog.take
    sql = insert_columns { o.note = "x"; o.save! }
    out << "create: validation=#{ChangeLog.take}"
    out << "create: saved=#{o.saved_changes.keys.sort.reject { |k| k.end_with?("_at") }.inspect} status=#{o.saved_change_to_status.inspect} quantity=#{o.saved_change_to_quantity.inspect} note=#{o.saved_change_to_note.inspect}"
    # Native Rails writes only the changed columns (partial_inserts); the
    # emitted INSERT lists every column, a difference this baseline does
    # not touch. What must hold is that the changed ones are written.
    out << "create: insert carries changed=#{%w[note status].all? { |c| sql.split(", ").include?(c) }}"
    o = Order.find(o.id)
    out << "reloaded: status=#{o.status} quantity=#{o.quantity} changed=#{o.changed.inspect}"
    o.update!(quantity: 2)
    out << "update: saved=#{o.saved_changes.keys.sort.reject { |k| k.end_with?("_at") }.inspect}"
    o = Order.create!(status: "open")
    out << "create!: status=#{o.saved_change_to_status.inspect} quantity=#{o.saved_change_to_quantity.inspect} changed=#{o.changed.inspect}"
    out
  end
end
"##;

const LOADS: [&str; 4] = [
    "app/models/change_log",
    "app/models/application_record",
    "app/models/order",
    "app/models/new_record_baseline_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/change_log.rb", CHANGE_LOG),
    ("app/models/order.rb", ORDER),
    ("app/models/new_record_baseline_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

const EXPECTED: [&str; 12] = [
    "bare: changed=[] changes={} status_was=\"start\"",
    "status: status_was=\"start\" changed=[\"status\"] changes={\"status\" => [\"start\", \"open\"]} changed?=true attr_was=\"start\"",
    "same as default: changed=[] has_changes=false",
    "mixed: changed=[\"note\", \"quantity\"] quantity=[0, 3] note=[nil, \"n\"] status_was=\"start\"",
    "written back: changed=[\"note\", \"quantity\"]",
    "writer: status_was=\"start\" changes={\"status\" => [\"start\", \"open\"]}",
    "create: validation=bv[{\"status\" => [\"start\", \"open\"], \"note\" => [nil, \"x\"]}]",
    "create: saved=[\"id\", \"note\", \"status\"] status=[\"start\", \"open\"] quantity=nil note=[nil, \"x\"]",
    "create: insert carries changed=true",
    "reloaded: status=open quantity=0 changed=[]",
    "update: saved=[\"quantity\"]",
    "create!: status=[\"start\", \"open\"] quantity=nil changed=[]",
];

#[test]
#[ignore = "fork spec: postgresql"]
fn new_record_dirty_baseline_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "new-record-baseline",
        &FILES,
        &emitted,
        "rh_spec_new_record_baseline",
        "NewRecordBaselineProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
