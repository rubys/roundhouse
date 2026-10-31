//! PostgreSQL: active record runtime callbacks, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE items (
    id bigserial NOT NULL,
    name character varying,
    note character varying,
    updated_at timestamp(6) without time zone,
    CONSTRAINT items_pkey PRIMARY KEY (id)
);

CREATE TABLE others (
    id bigserial NOT NULL,
    name character varying,
    note character varying,
    CONSTRAINT others_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_runtime_callbacks
"#;

/// The ActiveModel::Validations port translates through I18n, which the
/// lockfile names.
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

/// ShardedTable::InstanceMethods' shape, with one callback of each kind
/// the registration takes.
const GUARD: &str = r##"module Guard
  def self.included(base)
    base.before_create(:mark)
    base.after_find(:seen)
    base.before_validation(:guard_before_validation)
    base.after_validation { trail << "guard after_validation" }
    base.before_save(:guard_before_save)
    base.before_save(prepend: true) { trail << "guard prepend before_save" }
    base.around_save(:guard_around_save)
    base.after_save { |record| record.trail << "guard after_save new=#{record.new_record?}" }
    base.after_create(:guard_after_create, if: :loud?)
    base.before_update(:guard_before_update, unless: -> { name == "quiet" })
    base.after_update(:guard_after_update)
    base.before_destroy(:guard_before_destroy)
    base.after_destroy(:guard_after_destroy)
    base.after_commit(:guard_commit_create, on: :create)
    base.after_commit { trail << "guard after_commit" }
    base.after_destroy_commit(:guard_destroy_commit, unless: :keep?)
    base.after_rollback(:guard_rollback)
    base.after_touch(:guard_touch)
  end

  def trail
    @trail ||= []
  end

  def seen?
    @seen == true
  end

  def loud?
    name != "silent"
  end

  def keep?
    name == "keep"
  end

  private

  def mark
    trail << "guard mark"
    self.note = "x"
  end

  def seen
    @seen = true
    trail << "guard seen #{persisted?} #{name}"
  end

  def guard_before_validation
    trail << "guard before_validation"
    throw :abort if name == "halt-validation"
  end

  def guard_before_save
    trail << "guard before_save"
    throw :abort if name == "halt-save"
  end

  def guard_around_save
    trail << "guard around_save in"
    yield
    trail << "guard around_save out"
  end

  def guard_after_create
    trail << "guard after_create"
  end

  def guard_before_update
    trail << "guard before_update"
  end

  def guard_after_update
    trail << "guard after_update"
  end

  def guard_before_destroy
    trail << "guard before_destroy"
    throw :abort if name == "no-destroy"
  end

  def guard_after_destroy
    trail << "guard after_destroy"
  end

  def guard_commit_create
    trail << "guard commit on create"
  end

  def guard_destroy_commit
    trail << "guard after_destroy_commit"
  end

  def guard_rollback
    trail << "guard rollback"
  end

  def guard_touch
    trail << "guard after_touch"
  end
end
"##;

/// GlobalDB::ShopRecord's shape: an abstract class whose include
/// registers the callbacks every subclass inherits.
const SHOPISH: &str = r#"class Shopish < ApplicationRecord
  self.abstract_class = true
  include Guard
end
"#;

/// A concern the model includes itself, above its own callbacks.
const EXTRA: &str = r##"module Extra
  def self.included(base)
    base.before_create(:extra_mark)
    base.after_save(:extra_after_save)
  end

  private

  def extra_mark
    trail << "extra mark"
  end

  def extra_after_save
    trail << "extra after_save"
  end
end
"##;

const ITEM: &str = r##"class Item < Shopish
  include Extra
  before_validation :own_before_validation
  before_create :own
  after_save :own_after_save
  validates :name, presence: true

  private

  def own_before_validation
    trail << "item before_validation"
  end

  def own
    trail << "item before_create note=#{note}"
    throw :abort if name == "halt"
  end

  def own_after_save
    trail << "item after_save"
  end
end
"##;

const OTHER: &str = "class Other < Shopish\nend\n";

const CHECKABLE: &str = r#"module Checkable
  def self.included(base)
    base.include(ActiveModel::Validations)
    base.extend(ActiveRecord::Validations::ClassMethods)
  end

  def marked_for_destruction?
    false
  end
end
"#;

/// Script::EmbeddedEvent's shape.
const EVENT: &str = r#"CallbackDslEvent = Struct.new(:kind, :size, keyword_init: true) do
  include Checkable

  validates :kind, presence: true
  validates_length_of :kind, maximum: 5
end
"#;

/// Active Record's presence validator reads the class's association
/// reflection, which a Struct does not answer.
const ABSENT: &str = r#"CallbackDslAbsent = Struct.new(:size, keyword_init: true) do
  include Checkable

  validates_absence_of :size
end
"#;

const PROBE: &str = r##"class CallbackDslProbe
  def self.lines
    out = []
    a = Item.create!(name: "a")
    out << "create: #{a.note} #{a.trail.join(" | ")}"
    out << "seen after create: #{a.seen?}"
    first = Item.first
    out << "first: #{first.instance_variable_get(:@seen).inspect} #{first.trail.join(" | ")}"
    out << "find: #{Item.find(a.id).seen?} where: #{Item.where(name: "a").map(&:seen?).inspect}"
    a.trail.clear
    a.reload
    out << "reload: #{a.seen?} #{a.trail.inspect}"
    a.update!(name: "b")
    out << "update: #{a.trail.join(" | ")}"
    a.trail.clear
    a.update!(name: "quiet")
    out << "quiet update: #{a.trail.join(" | ")}"
    silent = Item.create!(name: "silent")
    out << "silent create: #{silent.trail.join(" | ")}"
    halt_save = Item.new(name: "halt-save")
    out << "halt save: #{halt_save.save.inspect} #{halt_save.persisted?} #{halt_save.trail.join(" | ")}"
    begin
      Item.create!(name: "halt")
    rescue ActiveRecord::RecordNotSaved => e
      out << "halt create!: #{e.class} #{e.message} #{e.record.trail.join(" | ")}"
    end
    halt_validation = Item.new(name: "halt-validation")
    out << "halt validation: #{halt_validation.save.inspect} #{halt_validation.trail.join(" | ")}"
    begin
      halt_validation.save!
    rescue ActiveRecord::RecordInvalid => e
      out << "halt validation save!: #{e.class}"
    end
    out << "count: #{Item.count}"
    a.trail.clear
    a.touch
    out << "touch: #{a.trail.join(" | ")}"
    a.trail.clear
    a.destroy
    out << "destroy: #{a.destroyed?} #{a.trail.join(" | ")}"
    keep = Item.create!(name: "keep")
    keep.trail.clear
    keep.destroy
    out << "keep destroy: #{keep.trail.join(" | ")}"
    kept = Item.create!(name: "no-destroy")
    kept.trail.clear
    out << "halt destroy: #{kept.destroy.inspect} #{kept.destroyed?} #{kept.trail.join(" | ")}"
    begin
      kept.destroy!
    rescue ActiveRecord::RecordNotDestroyed => e
      out << "halt destroy!: #{e.class} #{e.message} #{e.record.equal?(kept)}"
    end
    other = Other.create!(name: "o")
    out << "other: #{other.note} #{other.trail.join(" | ")}"
    out << "other find: #{Other.find(other.id).trail.join(" | ")}"
    # Persistence#save! answers `true` (create_or_update || raise), not the record.
    out << "save!: #{Other.new(name: "o2").save!.inspect}"
    event = CallbackDslEvent.new(kind: "toolong")
    # `errors.to_a` (Errors#full_messages' alias): lower::errors_full_messages
    # folds a literal `errors.full_messages` for a record it does not see
    # including ActiveModel::Validations, and an `included` hook's
    # `base.include` is not seen.
    out << "event: #{event.valid?} #{event.errors.to_a.inspect}"
    out << "event validators: #{CallbackDslEvent.validators.map { |v| v.class.name }.inspect}"
    out << "event ok: #{CallbackDslEvent.new(kind: "ok").valid?}"
    begin
      CallbackDslAbsent.new(size: 1).valid?
    rescue NoMethodError => e
      out << "absence: #{e.class} #{e.name}"
    end
    out
  end
end
"##;

const EXPECTED: [&str; 25] = [
    r#"create: x guard before_validation | item before_validation | guard after_validation | guard prepend before_save | guard before_save | guard around_save in | guard mark | extra mark | item before_create note=x | guard after_create | guard around_save out | guard after_save new=false | extra after_save | item after_save | guard after_commit | guard commit on create"#,
    r#"seen after create: false"#,
    r#"first: true guard seen true a"#,
    r#"find: true where: [true]"#,
    r#"reload: false []"#,
    r#"update: guard before_validation | item before_validation | guard after_validation | guard prepend before_save | guard before_save | guard around_save in | guard before_update | guard after_update | guard around_save out | guard after_save new=false | extra after_save | item after_save | guard after_commit"#,
    r#"quiet update: guard before_validation | item before_validation | guard after_validation | guard prepend before_save | guard before_save | guard around_save in | guard after_update | guard around_save out | guard after_save new=false | extra after_save | item after_save | guard after_commit"#,
    r#"silent create: guard before_validation | item before_validation | guard after_validation | guard prepend before_save | guard before_save | guard around_save in | guard mark | extra mark | item before_create note=x | guard around_save out | guard after_save new=false | extra after_save | item after_save | guard after_commit | guard commit on create"#,
    r#"halt save: false false guard before_validation | item before_validation | guard after_validation | guard prepend before_save | guard before_save"#,
    r#"halt create!: ActiveRecord::RecordNotSaved Failed to save the record guard before_validation | item before_validation | guard after_validation | guard prepend before_save | guard before_save | guard around_save in | guard mark | extra mark | item before_create note=x | guard around_save out"#,
    r#"halt validation: false guard before_validation"#,
    r#"halt validation save!: ActiveRecord::RecordInvalid"#,
    r#"count: 2"#,
    r#"touch: guard after_touch | guard after_commit"#,
    r#"destroy: true guard before_destroy | guard after_destroy | guard after_destroy_commit | guard after_commit"#,
    r#"keep destroy: guard before_destroy | guard after_destroy | guard after_commit"#,
    r#"halt destroy: false false guard before_destroy"#,
    r#"halt destroy!: ActiveRecord::RecordNotDestroyed Failed to destroy Item with id=4 true"#,
    r#"other: x guard before_validation | guard after_validation | guard prepend before_save | guard before_save | guard around_save in | guard mark | guard after_create | guard around_save out | guard after_save new=false | guard after_commit | guard commit on create"#,
    r#"other find: guard seen true o"#,
    r#"save!: true"#,
    r#"event: false ["Kind is too long (maximum is 5 characters)"]"#,
    r#"event validators: ["ActiveModel::Validations::PresenceValidator", "ActiveRecord::Validations::LengthValidator"]"#,
    r#"event ok: true"#,
    r#"absence: NoMethodError _reflect_on_association"#,
];

const FILES: [(&str, &str); 16] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/guard.rb", GUARD),
    ("app/models/shopish.rb", SHOPISH),
    ("app/models/extra.rb", EXTRA),
    ("app/models/item.rb", ITEM),
    ("app/models/other.rb", OTHER),
    ("app/models/checkable.rb", CHECKABLE),
    ("app/models/callback_dsl_event.rb", EVENT),
    ("app/models/callback_dsl_absent.rb", ABSENT),
    ("app/models/callback_dsl_probe.rb", PROBE),
    ("config/application.rb", "module CallbackDsl\n  class Application < Rails::Application\n  end\nend\n"),
];

const LOADS: [&str; 10] = [
    "app/models/application_record",
    "app/models/guard",
    "app/models/shopish",
    "app/models/extra",
    "app/models/item",
    "app/models/other",
    "app/models/checkable",
    "app/models/callback_dsl_event",
    "app/models/callback_dsl_absent",
    "app/models/callback_dsl_probe",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

/// The PostgreSQL tree carries the callbacks runtime after
/// active_record_core.rb, whose Persistence it reopens, and
/// ActiveRecord::Validations after the ActiveModel::Validations port it
/// builds on; the concerns' runtime calls are emitted as written.
#[test]
#[ignore = "fork spec: postgresql"]
fn postgresql_tree_carries_the_callback_and_validations_runtime() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
    let facades = read("runtime/gem_facades.rb");
    let at = |feature: &str| {
        facades
            .find(&format!("require_relative \"{feature}\"\n"))
            .unwrap_or_else(|| panic!("gem_facades.rb requires {feature}:\n{facades}"))
    };
    assert!(at("active_model_validations") < at("active_record_validations"), "{facades}");
    assert!(at("active_record_core") < at("active_record_callbacks"), "{facades}");
    assert!(read("runtime/active_record_callbacks.rb").contains("module Callbacks"));
    assert!(read("runtime/active_record_validations.rb").contains("module Validations"));
    let checkable = read("app/models/checkable.rb");
    assert!(checkable.contains("base.extend(ActiveRecord::Validations::ClassMethods)"), "{checkable}");
    assert!(!checkable.contains("not supported"), "{checkable}");
    let guard = read("app/models/guard.rb");
    assert!(guard.contains("base.before_create(:mark)") && guard.contains("base.after_find(:seen)"), "{guard}");
}

/// The SQLite CRuby tree carries neither port: it refuses the read by the
/// name and file that declare it.
#[test]
#[ignore = "fork spec: postgresql"]
fn sqlite_tree_refuses_active_record_validations_by_name() {
    let app = FILES
        .iter()
        .filter(|(path, _)| *path != "roundhouse.yml")
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (_emitted, errors) = app.emit(roundhouse::project::BuildTarget::Ruby);
    assert!(
        errors.iter().any(|e| e.contains("ActiveRecord::Validations::ClassMethods")
            && e.contains("runtime/active_record_validations.rb")),
        "{}",
        errors.join("\n")
    );
}

#[test]
#[ignore = "fork spec: postgresql"]
fn runtime_registered_callbacks_match_native_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "active-record-runtime-callbacks",
        &FILES,
        &emitted,
        "rh_spec_runtime_callbacks",
        "CallbackDslProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
