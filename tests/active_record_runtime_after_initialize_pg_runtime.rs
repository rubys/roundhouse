//! PostgreSQL: active record runtime after initialize, on the Ruby target against native Rails.
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
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);

CREATE TABLE gadgets (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_runtime_after_initialize
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// audit_trail's shape: callbacks registered on the class from outside
/// its body, one of them a block taking the record.
const TRACKABLE: &str = r##"module Trackable
  def self.included(base)
    base.after_initialize do |object|
      object.trail << "init new=#{object.new_record?} name=#{object.name.inspect}" if object.new_record? || object.name
    end
    base.after_initialize(:named_init, if: :named?)
    base.after_find { trail << "find" }
  end

  def trail
    @trail ||= []
  end

  def named?
    !name.nil?
  end

  private

  def named_init
    trail << "named #{name}"
  end
end
"##;

const TRACKED: &str = r#"class Tracked < ApplicationRecord
  self.abstract_class = true
  include Trackable
end
"#;

const WIDGET: &str = "class Widget < Tracked\nend\n";

/// A model with its own declared hook, below the inherited ones.
const GADGET: &str = r##"class Gadget < Tracked
  after_initialize :own_init

  private

  def own_init
    trail << "own new=#{new_record?}"
  end
end
"##;

const PROBE: &str = r##"class AfterInitializeProbe
  def self.lines
    out = []
    out << "new: #{Widget.new(name: "a").trail.join(" | ")}"
    out << "bare new: #{Widget.new.trail.join(" | ")}"
    w = Widget.create!(name: "c")
    out << "create: #{w.trail.join(" | ")}"
    out << "find: #{Widget.find(w.id).trail.join(" | ")}"
    out << "where: #{Widget.where(name: "c").map { |r| r.trail.join(" | ") }.inspect}"
    out << "first: #{Widget.first.trail.join(" | ")}"
    w.trail.clear
    w.reload
    out << "reload: #{w.trail.inspect}"
    out << "gadget new: #{Gadget.new(name: "g").trail.join(" | ")}"
    g = Gadget.create!
    out << "gadget create: #{g.trail.join(" | ")}"
    out << "gadget find: #{Gadget.find(g.id).trail.join(" | ")}"
    out << "gadget all: #{Gadget.all.to_a.map { |r| r.trail.join(" | ") }.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 11] = [
    r#"new: init new=true name="a" | named a"#,
    r#"bare new: init new=true name=nil"#,
    r#"create: init new=true name="c" | named c"#,
    r#"find: find | init new=false name="c" | named c"#,
    r#"where: ["find | init new=false name=\"c\" | named c"]"#,
    r#"first: find | init new=false name="c" | named c"#,
    r#"reload: []"#,
    r#"gadget new: init new=true name="g" | named g | own new=true"#,
    r#"gadget create: init new=true name=nil | own new=true"#,
    r#"gadget find: find | own new=false"#,
    r#"gadget all: ["find | own new=false"]"#,
];

const FILES: [(&str, &str); 10] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/trackable.rb", TRACKABLE),
    ("app/models/tracked.rb", TRACKED),
    ("app/models/widget.rb", WIDGET),
    ("app/models/gadget.rb", GADGET),
    ("app/models/after_initialize_probe.rb", PROBE),
];

const LOADS: [&str; 6] = [
    "app/models/application_record",
    "app/models/trackable",
    "app/models/tracked",
    "app/models/widget",
    "app/models/gadget",
    "app/models/after_initialize_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The concern's runtime registrations are emitted as written, and the
/// runtime takes them rather than refusing.
#[test]
#[ignore = "fork spec: postgresql layout"]
fn postgresql_tree_takes_runtime_after_initialize() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let trackable = read("app/models/trackable.rb");
    assert!(trackable.contains("base.after_initialize(:named_init, if: :named?)"), "{trackable}");
    let callbacks = read("runtime/active_record_callbacks.rb");
    assert!(callbacks.contains("module InitializeHooks"), "{callbacks}");
    assert!(!callbacks.contains("after_initialize called at runtime"), "{callbacks}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn runtime_after_initialize_matches_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-record-runtime-after-initialize",
        &FILES,
        &emitted,
        "rh_spec_runtime_after_initialize",
        "AfterInitializeProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
