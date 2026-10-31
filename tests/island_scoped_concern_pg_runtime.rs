//! PostgreSQL: island scoped concern, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE cabinets (
    id bigserial NOT NULL,
    CONSTRAINT cabinets_pkey PRIMARY KEY (id)
);

CREATE TABLE gadgets (
    id bigserial NOT NULL,
    cabinet_id bigint,
    name character varying,
    island integer NOT NULL,
    CONSTRAINT gadgets_pkey PRIMARY KEY (id)
);

CREATE TABLE taggings (
    id bigserial NOT NULL,
    taggable_id bigint,
    taggable_type character varying NOT NULL,
    CONSTRAINT taggings_pkey PRIMARY KEY (id)
);

CREATE TABLE widgets (
    id bigserial NOT NULL,
    name character varying,
    island integer NOT NULL,
    CONSTRAINT widgets_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_island_scoped
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const CURRENT: &str = r#"module IslandCurrent
  def self.island_id
    7
  end

  def self.enabled?
    ENV.fetch("ISLAND_MODE", "on") == "on"
  end
end
"#;

const CONCERN: &str = r#"module IslandScoped
  extend ActiveSupport::Concern

  module RelationMethods
    def island_tag
      "island"
    end
  end

  included do
    if IslandCurrent.enabled?
      default_scope -> { extending(RelationMethods).where(island: IslandCurrent.island_id) }, all_queries: true
      scope :all_islands, -> { unscope(where: :island) }
    else
      scope :all_islands, -> { all }
    end
  end

  class_methods do
    def insert_all(rows)
      super(rows.map { |row| row.key?(:island) ? row : row.merge(island: IslandCurrent.island_id) })
    end
  end
end
"#;

const ISLAND_RECORD: &str = r#"class IslandRecord < ApplicationRecord
  self.abstract_class = true
  include IslandScoped
end
"#;

const GADGET: &str = r#"class Gadget < IslandRecord
  belongs_to :cabinet, optional: true
  has_many :taggings, as: :taggable
end
"#;

/// Only Gadget declares `as: :taggable`; Widget takes the same writer.
const TAGGING: &str = r#"class Tagging < ApplicationRecord
  belongs_to :taggable, polymorphic: true
end
"#;

const CABINET: &str = r#"class Cabinet < ApplicationRecord
  has_many :gadgets
end
"#;

const WIDGET: &str = r#"class Widget < ApplicationRecord
  include IslandScoped
end
"#;

const PROBE: &str = r##"class IslandScopedProbe
  def self.lines
    out = []
    cabinet = Cabinet.create!
    g = Gadget.new(name: "a")
    out << "new: #{g.island.inspect} #{Gadget.new.island.inspect} #{Gadget.unscoped.new.island.inspect} #{Gadget.all_islands.build.island.inspect}"
    g.save!
    w = Widget.create!(name: "w")
    out << "create: #{Gadget.count} #{Gadget.first.island} #{w.island} #{Widget.count}"
    built = cabinet.gadgets.build(name: "b")
    made = cabinet.gadgets.create!(name: "c")
    out << "assoc: #{built.island} #{built.cabinet_id == cabinet.id} #{made.island} #{cabinet.gadgets.count}"
    out << "where-new: #{Gadget.where(island: 9).new.island} #{Gadget.where(name: "q").new.island} #{Gadget.where(name: "q").new.name} #{Gadget.where(island: 1..3).new.island.inspect}"
    Gadget.insert_all([{ name: "i" }])
    out << "insert_all: #{Gadget.where(name: "i").pluck(:island).inspect}"
    out << "all_islands: #{Gadget.all_islands.count} #{Gadget.all.island_tag} #{Gadget.where(name: "a").island_tag}"
    out << "extending: #{Widget.all.island_tag} #{Widget.all_islands.count}"
    on_widget = Tagging.new(taggable: w)
    on_gadget = Tagging.new(taggable: g)
    Tagging.create!(taggable: w)
    out << "polymorphic: #{on_widget.taggable_type} #{on_widget.taggable_id == w.id} #{on_gadget.taggable_type} #{Tagging.count}"
    out
  end
end
"##;

const EXPECTED: [&str; 8] = [
    "new: 7 7 nil nil",
    "create: 1 7 7 1",
    "assoc: 7 true 7 1",
    "where-new: 9 7 q 7",
    "insert_all: [7]",
    "all_islands: 3 island island",
    "extending: island 1",
    "polymorphic: Widget true Gadget 1",
];

const LOADS: [&str; 9] = [
    "app/models/application_record",
    "app/models/island_current",
    "app/models/island_scoped",
    "app/models/island_record",
    "app/models/cabinet",
    "app/models/gadget",
    "app/models/widget",
    "app/models/tagging",
    "app/models/island_scoped_probe",
];

const FILES: [(&str, &str); 14] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/island_current.rb", CURRENT),
    ("app/models/island_scoped.rb", CONCERN),
    ("app/models/island_record.rb", ISLAND_RECORD),
    ("app/models/cabinet.rb", CABINET),
    ("app/models/gadget.rb", GADGET),
    ("app/models/widget.rb", WIDGET),
    ("app/models/tagging.rb", TAGGING),
    ("app/models/island_scoped_probe.rb", PROBE),
    ("app/controllers/application_controller.rb", "class ApplicationController < ActionController::Base\nend\n"),
    ];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

#[test]
#[ignore = "fork spec: postgresql"]
fn island_scoped_concern_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("island-scoped", &FILES, &emitted, "rh_spec_island_scoped", "IslandScopedProbe", &LOADS) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}

/// No database needed: the conditional arms of the concern's `included do`
/// are gated, not dropped (`lower_residue` named the `<If>` and the class
/// had no default scope), and the polymorphic writer stores the type of
/// a class no `as:` names.
#[test]
#[ignore = "fork spec: postgresql"]
fn conditional_default_scope_is_emitted_gated() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |name: &str| std::fs::read_to_string(emitted.join(name)).expect(name);
    let island_record = read("app/models/island_record.rb");
    assert!(island_record.contains("def self._default_scope_guard_islandrecord_0"), "{island_record}");
    assert!(island_record.contains("super || _default_scope_guard_islandrecord_0"), "{island_record}");
    assert!(island_record.contains("where!(island: IslandCurrent.island_id)"), "{island_record}");
    assert!(island_record.contains("def self.all_islands"), "{island_record}");
    let widget = read("app/models/widget.rb");
    assert!(widget.contains("_default_scope_guard_widget_0"), "a direct includer gets its own guard");
    let tagging = read("app/models/tagging.rb");
    assert!(tagging.contains("polymorphic_name"), "{tagging}");
}
