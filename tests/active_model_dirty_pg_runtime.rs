//! PostgreSQL: active model dirty, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_model_dirty
"#;

const STRUCTURE: &str = "CREATE TABLE items (\n    id bigserial NOT NULL,\n    name character varying,\n    CONSTRAINT items_pkey PRIMARY KEY (id)\n);\n\nCREATE TABLE watcheds (\n    id bigserial NOT NULL,\n    name character varying,\n    CONSTRAINT watcheds_pkey PRIMARY KEY (id)\n);\n";

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const ITEM: &str = r#"class Item < ApplicationRecord
end
"#;

const WATCHED: &str = r##"class Watched < ApplicationRecord
  before_save { trail << "before_save"; throw :abort if name == "halt" }
  after_save { trail << "after_save" }
  after_commit { trail << "after_commit" }

  def trail
    @trail ||= []
  end
end
"##;

const PLACEMENT: &str = r#"class Placement
  include ActiveModel::Attributes
  include ActiveModel::Dirty

  attribute :name, :string
  attribute :count, :integer, default: 1
  attribute :active, :boolean

  attr_accessor :extra

  def changed?
    super || !extra.nil?
  end

  def apply
    changes_applied
  end
end
"#;

const SUB_PLACEMENT: &str = r#"class SubPlacement < Placement
  attribute :label, :string, default: "none"
end
"#;

const TICKETISH: &str = r##"class Ticketish
  include ActiveModel::Dirty

  FIELDS = ["subject", "status"].freeze

  define_attribute_methods(*FIELDS)

  attr_reader(*FIELDS)

  def subject=(value)
    subject_will_change! unless value == @subject
    @subject = value
  end

  def status=(value)
    send("status_will_change!") unless value == @status
    @status = value
  end

  def changed_values
    changed.to_h { |field| [field, public_send(field)] }
  end

  def refresh
    changes_applied
  end
end
"##;

const PROBE: &str = r##"class ModelDirtyProbe
  def self.lines
    out = []
    p = Placement.new
    out << "fresh: #{p.changed?} #{p.changed.inspect} #{p.changes.to_h.inspect}"
    p.name = "a"
    out << "name: #{p.changed?} #{p.changed.inspect} #{p.changes.to_h.inspect} #{p.name_changed?} #{p.name_was.inspect} #{p.name_change.inspect} #{p.count_changed?}"
    out << "from/to: #{p.attribute_changed?(:name, from: nil, to: "a")} #{p.attribute_changed?(:name, to: "b")} #{p.name_changed?(from: "z")}"
    p.count = "5"
    out << "count: #{p.count.inspect} #{p.count_change.inspect} #{p.changed_attributes.to_h.inspect} #{p.attribute_was(:count).inspect}"
    p.name = nil
    out << "reverted: #{p.name_changed?} #{p.changed.inspect}"
    p.name = "b"
    p.extra = 1
    out << "super chain: #{p.changed?}"
    p.extra = nil
    p.apply
    out << "applied: #{p.changed?} #{p.changed.inspect} #{p.previous_changes.to_h.inspect} #{p.name_previously_changed?} #{p.name_previously_was.inspect} #{p.name_previous_change.inspect} #{p.active_previously_changed?}"
    p.name = "c"
    p.active = "1"
    out << "second: #{p.changes.to_h.inspect} #{p.attribute_previously_changed?(:name)}"
    p.restore_attributes
    out << "restored: #{p.name.inspect} #{p.active.inspect} #{p.changed?}"
    p.count = 9
    p.restore_count!
    p.name = "d"
    p.clear_name_change
    out << "cleared: #{p.count} #{p.name} #{p.changed.inspect}"
    p.name = "e"
    copy = p.dup
    out << "dup: #{copy.name} #{copy.changed.inspect} #{p.changed.inspect}"
    copy.name = "f"
    out << "dup isolated: #{p.name} #{copy.name} #{p.name_was.inspect} #{copy.name_was.inspect}"
    p.clear_changes_information
    out << "info cleared: #{p.changed?} #{p.previous_changes.to_h.inspect}"
    sub = SubPlacement.new
    sub.label = "x"
    out << "sub: #{sub.changed.inspect} #{sub.label_changed?} #{sub.label_was.inspect} #{Placement.new.respond_to?(:label_changed?)}"

    t = Ticketish.new
    t.subject = "hello"
    t.status = "new"
    out << "plain: #{t.changed.inspect} #{t.changes.to_h.inspect} #{t.subject_changed?} #{t.subject_was.inspect} #{t.changed_values.inspect}"
    t.subject = "hello"
    t.subject = "bye"
    out << "plain twice: #{t.subject_change.inspect} #{t.status_was.inspect}"
    t.refresh
    out << "plain applied: #{t.changed?} #{t.changes.to_h.inspect} #{t.previous_changes.to_h.inspect} #{t.subject_previously_changed?}"
    t.subject = "again"
    out << "plain next: #{t.changed.inspect} #{t.subject_was.inspect} #{t.previous_changes.keys.inspect}"
    out
  end
end
"##;

const CALLBACKS_PROBE: &str = r##"class RunCallbacksProbe
  def self.lines
    out = []
    item = Item.new
    out << "none: #{item.run_callbacks(:save).inspect} #{item.run_callbacks(:save) { :body }.inspect} #{item.run_callbacks(:save, :before).inspect} #{item.run_callbacks(:commit).inspect} #{item.run_callbacks(:destroy) { 1 }.inspect}"
    w = Watched.new
    out << "all: #{w.run_callbacks(:save).inspect} #{w.trail.inspect}"
    w = Watched.new
    out << "block: #{w.run_callbacks(:save) { w.trail << "body"; :done }.inspect} #{w.trail.inspect}"
    w = Watched.new
    out << "before: #{w.run_callbacks(:save, :before).inspect} #{w.trail.inspect}"
    w = Watched.new
    out << "after: #{w.run_callbacks(:save, :after).inspect} #{w.trail.inspect}"
    w = Watched.new
    out << "halted: #{(w.name = "halt"; w.run_callbacks(:save) { :body }).inspect} #{w.trail.inspect}"
    w = Watched.new
    out << "commit: #{w.run_callbacks(:commit).inspect} #{w.trail.inspect}"
    w = Watched.new(name: "ok")
    w.save!
    out << "saved: #{w.trail.inspect}"
    out
  end
end
"##;

const DIRTY_EXPECTED: [&str; 18] = [
    "fresh: false [] {}",
    "name: true [\"name\"] {\"name\" => [nil, \"a\"]} true nil [nil, \"a\"] false",
    "from/to: true false false",
    "count: 5 [1, 5] {\"name\" => nil, \"count\" => 1} 1",
    "reverted: false [\"count\"]",
    "super chain: true",
    "applied: false [] {\"name\" => [nil, \"b\"], \"count\" => [1, 5]} true nil [nil, \"b\"] false",
    "second: {\"name\" => [\"b\", \"c\"], \"active\" => [nil, true]} true",
    "restored: \"b\" nil false",
    "cleared: 5 d []",
    "dup: e [\"name\"] [\"name\"]",
    "dup isolated: e f \"d\" \"d\"",
    "info cleared: false {}",
    "sub: [\"label\"] true \"none\" false",
    "plain: [\"subject\", \"status\"] {\"subject\" => [nil, \"hello\"], \"status\" => [nil, \"new\"]} true nil {\"subject\" => \"hello\", \"status\" => \"new\"}",
    "plain twice: [nil, \"bye\"] nil",
    "plain applied: false {} {\"subject\" => [nil, \"bye\"], \"status\" => [nil, \"new\"]} true",
    "plain next: [\"subject\"] \"bye\" [\"subject\", \"status\"]",
];
const CALLBACKS_EXPECTED: [&str; 8] = [
    "none: nil :body nil nil 1",
    "all: true [\"before_save\", \"after_save\"]",
    "block: :done [\"before_save\", \"body\", \"after_save\"]",
    "before: true [\"before_save\"]",
    "after: true [\"after_save\"]",
    "halted: false [\"before_save\"]",
    "commit: true [\"after_commit\"]",
    "saved: [\"before_save\", \"after_save\", \"after_commit\"]",
];

const FILES: [(&str, &str); 12] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/item.rb", ITEM),
    ("app/models/watched.rb", WATCHED),
    ("app/models/placement.rb", PLACEMENT),
    ("app/models/sub_placement.rb", SUB_PLACEMENT),
    ("app/models/ticketish.rb", TICKETISH),
    ("app/models/model_dirty_probe.rb", PROBE),
];

const LOADS: [&str; 8] = [
    "app/models/application_record",
    "app/models/item",
    "app/models/watched",
    "app/models/placement",
    "app/models/sub_placement",
    "app/models/ticketish",
    "app/models/model_dirty_probe",
    "app/models/run_callbacks_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES
        .iter()
        .chain([&("app/models/run_callbacks_probe.rb", CALLBACKS_PROBE)])
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn files() -> Vec<(&'static str, &'static str)> {
    FILES.iter().copied().chain([("app/models/run_callbacks_probe.rb", CALLBACKS_PROBE)]).collect()
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The tree carries the Dirty port over the AttributeMethods port it
/// includes, after the Attributes port it follows.
#[test]
#[ignore = "fork spec: postgresql"]
fn postgresql_tree_carries_the_dirty_port() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let facades = read("runtime/gem_facades.rb");
    let at = |feature: &str| {
        facades
            .find(&format!("require_relative \"{feature}\"\n"))
            .unwrap_or_else(|| panic!("gem_facades.rb requires {feature}:\n{facades}"))
    };
    assert!(at("active_model_attribute_methods") < at("active_model_dirty"), "{facades}");
    assert!(at("active_model_attributes") < at("active_model_dirty"), "{facades}");
    assert!(read("runtime/active_model_dirty.rb").contains("module Dirty"));
    assert!(read("app/models/placement.rb").contains("include ActiveModel::Dirty"), "the class keeps its include");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn dirty_tracking_matches_native_rails() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "model-dirty",
        &files(),
        &emitted,
        "rh_spec_model_dirty",
        "ModelDirtyProbe",
        &LOADS[..7],
    ) else { return };
    assert_eq!(native, DIRTY_EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn run_callbacks_matches_native_rails() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "run-callbacks",
        &files(),
        &emitted,
        "rh_spec_run_callbacks",
        "RunCallbacksProbe",
        &[LOADS[0], LOADS[1], LOADS[2], LOADS[7]],
    ) else { return };
    assert_eq!(native, CALLBACKS_EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
