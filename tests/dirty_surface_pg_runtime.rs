//! PostgreSQL: dirty surface, on the Ruby target against native Rails.
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
    count integer,
    created_at timestamp(6) without time zone,
    updated_at timestamp(6) without time zone,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_dirty_surface
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

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

const WIDGET: &str = r##"class Widget < ApplicationRecord
  before_save :log_before
  after_save :log_after_save
  after_commit :log_commit

  def log_before
    ChangeLog.add("before[#{pending}]")
  end

  def force_name
    name_will_change!
  end

  def id_saved?
    saved_change_to_id?
  end

  def log_after_save
    ChangeLog.add("after_save[#{saved}]")
  end

  def log_commit
    ChangeLog.add("commit[#{saved}]")
  end

  def pending
    "will=#{will_save_change_to_attribute?(:name)} to_save=#{has_changes_to_save?} " \
      "names=#{changed_attribute_names_to_save.sort.join("/")} name_to_be_saved=#{attribute_change_to_be_saved(:name).inspect} " \
      "in_db=#{attribute_in_database(:name).inspect} from_nil=#{will_save_change_to_attribute?("name", from: nil)} " \
      "to_a=#{will_save_change_to_attribute?(:name, to: "a")} to_z=#{will_save_change_to_attribute?(:name, to: "z")}"
  end

  def saved
    "any=#{saved_changes?} name=#{saved_change_to_attribute?(:name)} before=#{attribute_before_last_save(:name).inspect} " \
      "pair=#{saved_change_to_attribute(:name).inspect} prev_was=#{attribute_previously_was(:name).inspect} " \
      "prev_changed=#{attribute_previously_changed?(:name)} " \
      "from_to=#{saved_change_to_attribute?(:name, from: "a", to: "b")} from_x=#{saved_change_to_attribute?(:name, from: "x")} " \
      "count=#{saved_change_to_attribute?(:count, to: 2)}"
  end
end
"##;

const PROBE: &str = r##"class DirtySurfaceProbe
  def self.lines
    out = []
    w = Widget.new
    w.name = "a"
    w.count = 1
    w.save!
    out << "create: #{ChangeLog.take}"
    out << "saved_change_to_id after create: #{w.id_saved?} #{w.saved_change_to_id.inspect == [nil, w.id].inspect}"
    w.update!(name: "b")
    out << "update name: #{ChangeLog.take}"
    out << "saved_change_to_id after update: #{w.id_saved?} #{w.saved_change_to_id.inspect}"
    w.update!(count: 2)
    out << "update count: #{ChangeLog.take}"
    w = Widget.find(w.id)
    out << "reloaded: any=#{w.saved_changes?} to_save=#{w.has_changes_to_save?} name=#{w.saved_change_to_attribute?(:name)}"
    w.name = "c"
    out << "dirty: changed=#{w.changed.inspect} changes=#{w.changes.inspect} attrs=#{w.changed_attributes.inspect}"
    out << "dirty: in_db=#{w.attribute_in_database("name").inspect} was=#{w.attribute_was(:name).inspect} to_save=#{w.changes_to_save.inspect}"
    w.restore_attributes
    out << "restored: name=#{w.name.inspect} changed=#{w.changed.inspect}"
    w.name = "d"
    w.count = 9
    w.restore_attributes([:count])
    out << "partial restore: name=#{w.name} count=#{w.count} changed=#{w.changed.inspect}"
    w.clear_attribute_changes([:name])
    out << "cleared: changed=#{w.changed.inspect}"
    w.name = "e"
    w.save!
    w.name = "f"
    w.reload
    out << "after reload: name=#{w.name} any=#{w.saved_changes?} changed=#{w.changed.inspect}"
    w.save!
    out << "unchanged save: any=#{w.saved_changes?} prev_was=#{w.attribute_previously_was(:name)}"
    w.force_name
    out << "will_change!: changed=#{w.changed.inspect} to_save=#{w.changes_to_save.inspect}"
    ChangeLog.take
    w.save!
    out << "forced save: any=#{w.saved_changes?} name=#{w.saved_change_to_attribute?(:name)} pair=#{w.saved_change_to_attribute(:name).inspect} changed=#{w.changed.inspect}"
    w.save!
    out << "forced mark dropped: any=#{w.saved_changes?} changed=#{w.changed.inspect}"
    out
  end
end
"##;

const LOADS: [&str; 4] = [
    "app/models/change_log",
    "app/models/application_record",
    "app/models/widget",
    "app/models/dirty_surface_probe",
];

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/change_log.rb", CHANGE_LOG),
    ("app/models/widget.rb", WIDGET),
    ("app/models/dirty_surface_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

const EXPECTED: [&str; 16] = [
    "create: before[will=true to_save=true names=count/name name_to_be_saved=[nil, \"a\"] in_db=nil from_nil=true to_a=true to_z=false] after_save[any=true name=true before=nil pair=[nil, \"a\"] prev_was=nil prev_changed=true from_to=false from_x=false count=false] commit[any=true name=true before=nil pair=[nil, \"a\"] prev_was=nil prev_changed=true from_to=false from_x=false count=false]",
    "saved_change_to_id after create: true true",
    "update name: before[will=true to_save=true names=name name_to_be_saved=[\"a\", \"b\"] in_db=\"a\" from_nil=false to_a=false to_z=false] after_save[any=true name=true before=\"a\" pair=[\"a\", \"b\"] prev_was=\"a\" prev_changed=true from_to=true from_x=false count=false] commit[any=true name=true before=\"a\" pair=[\"a\", \"b\"] prev_was=\"a\" prev_changed=true from_to=true from_x=false count=false]",
    "saved_change_to_id after update: false nil",
    "update count: before[will=false to_save=true names=count name_to_be_saved=nil in_db=\"b\" from_nil=false to_a=false to_z=false] after_save[any=true name=false before=\"b\" pair=nil prev_was=\"b\" prev_changed=false from_to=false from_x=false count=true] commit[any=true name=false before=\"b\" pair=nil prev_was=\"b\" prev_changed=false from_to=false from_x=false count=true]",
    "reloaded: any=false to_save=false name=false",
    "dirty: changed=[\"name\"] changes={\"name\" => [\"b\", \"c\"]} attrs={\"name\" => \"b\"}",
    "dirty: in_db=\"b\" was=\"b\" to_save={\"name\" => [\"b\", \"c\"]}",
    "restored: name=\"b\" changed=[]",
    "partial restore: name=d count=2 changed=[\"name\"]",
    "cleared: changed=[]",
    "after reload: name=e any=false changed=[]",
    "unchanged save: any=false prev_was=e",
    "will_change!: changed=[\"name\"] to_save={\"name\" => [\"e\", \"e\"]}",
    "forced save: any=true name=true pair=[\"e\", \"e\"] changed=[]",
    "forced mark dropped: any=false changed=[]",
];

#[test]
#[ignore = "fork spec: postgresql"]
fn dirty_surface_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:?}");
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "dirty-surface",
        &FILES,
        &emitted,
        "rh_spec_dirty_surface",
        "DirtySurfaceProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}
