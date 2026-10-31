//! PostgreSQL: model class body mixins reflections, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

const STRUCTURE: &str = r#"CREATE TABLE accounts (
    id bigserial NOT NULL,
    name character varying,
    code character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
CREATE TABLE notes (
    id bigserial NOT NULL,
    owner_id bigint,
    owner_type character varying,
    body character varying,
    CONSTRAINT notes_pkey PRIMARY KEY (id)
);
CREATE TABLE ledgers (
    id bigserial NOT NULL,
    name character varying,
    CONSTRAINT ledgers_pkey PRIMARY KEY (id)
);
"#;

const DATABASE_YML: &str = r#"default: &default
  adapter: postgresql
  encoding: unicode

test:
  <<: *default
  database: rh_spec_class_body_mixins
"#;

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const SENSITIVE_FLAGS: &str = r#"module SensitiveFlags
  def validate_sensitive(*names)
    (@sensitive_names ||= []).concat(names)
    before_validation(*names)
  end

  def sensitive_names
    @sensitive_names
  end
end
"#;

const PASSWORD_PROTECTION: &str = r#"module PasswordProtection
  module ClassMethods
    def protected_by_password?
      true
    end
  end

  class << self
    def included(base)
      base.validate_sensitive(:fill_code)
      base.extend(ClassMethods)
    end
  end

  def fill_code
    self.code ||= "filled:#{name}"
  end
end
"#;

const HAS_NOTES: &str = r#"module HasNotes
  class << self
    def included(base)
      base.has_many(
        :notes,
        as: :owner,
        inverse_of: "owner",
        autosave: false,
        foreign_key: "owner_id"
      )
      base.reflect_on_association(:notes).options[:autosave] = false
    end
  end
end
"#;

const ACCOUNT: &str = r#"class Account < ApplicationRecord
  extend SensitiveFlags
  include PasswordProtection
  include HasNotes
end
"#;

const NOTE: &str = r#"class Note < ApplicationRecord
  belongs_to :owner, polymorphic: true
end
"#;

const FILTER_CONDITIONS: &str = r##"module FilterConditions
  def since_id_filter(params)
    return unless params[:since_id]

    "#{table_name}.id > #{Integer(params[:since_id].to_s, 10)}"
  end
end
"##;

const PARAM_FILTERS: &str = r##"module ParamFilters
  extend ActiveSupport::Concern

  included do
    cattr_accessor :param_filter_methods
    self.param_filter_methods = []
  end

  module ClassMethods
    include FilterConditions

    def conditions_from_params(params = {})
      param_filter_methods.map { |method| send(method, params) }.compact.join(" AND ")
    end

    def param_filters(*args)
      missing = args.map { |m| m unless respond_to?(m) }.compact.join(", ")
      if missing.present?
        raise ArgumentError, "There are no methods named (#{missing}) found in FilterConditions"
      end

      self.param_filter_methods = (param_filter_methods + args)
    end
  end
end
"##;

const LEDGER: &str = r#"class Ledger < ApplicationRecord
  include ParamFilters

  param_filters :since_id_filter
end
"#;

const PROBE: &str = r##"class MixinsProbe
  def self.lines
    out = []
    a = Account.create!(name: "a")
    out << "account: #{Account.sensitive_names.inspect} #{Account.protected_by_password?} #{a.code}"
    r = Account.reflect_on_association(:notes)
    out << "notes: #{r.name.inspect} #{r.macro.inspect} #{r.class_name} #{r.collection?} #{r.options.inspect}"
    r.options[:autosave] = true
    again = Account.reflect_on_association("notes")
    out << "shared: #{again.equal?(r)} #{again.options[:autosave].inspect}"
    out << "missing: #{Account.reflect_on_association(:nope).inspect} #{Ledger.reflect_on_association(:notes).inspect}"
    owner = Note.reflect_on_association(:owner)
    out << "owner: #{owner.macro.inspect} #{owner.polymorphic?.inspect} #{owner.foreign_type} #{owner.options.inspect}"
    note = Note.new(body: "n")
    note.owner = a
    note.save!
    out << "loaded: #{Account.find(a.id).notes.map(&:body).inspect}"
    out << "ledger: #{Ledger.respond_to?(:since_id_filter)} #{Ledger.param_filter_methods.inspect} #{Ledger.conditions_from_params(since_id: "5")}"
    out
  end
end
"##;

const EXPECTED: [&str; 7] = [
    r#"account: [:fill_code] true filled:a"#,
    r#"notes: :notes :has_many Note true {as: :owner, inverse_of: "owner", autosave: false, foreign_key: "owner_id"}"#,
    r#"shared: true true"#,
    r#"missing: nil nil"#,
    r#"owner: :belongs_to true owner_type {polymorphic: true}"#,
    r#"loaded: ["n"]"#,
    r#"ledger: true [:since_id_filter] ledgers.id > 5"#,
];

/// What native Rails requires, in order.
const LOADS: [&str; 10] = [
    "app/models/application_record",
    "app/models/sensitive_flags",
    "app/models/password_protection",
    "app/models/has_notes",
    "app/models/filter_conditions",
    "app/models/param_filters",
    "app/models/account",
    "app/models/note",
    "app/models/ledger",
    "app/models/mixins_probe",
];

const FILES: [(&str, &str); 14] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/sensitive_flags.rb", SENSITIVE_FLAGS),
    ("app/models/password_protection.rb", PASSWORD_PROTECTION),
    ("app/models/has_notes.rb", HAS_NOTES),
    ("app/models/filter_conditions.rb", FILTER_CONDITIONS),
    ("app/models/param_filters.rb", PARAM_FILTERS),
    ("app/models/account.rb", ACCOUNT),
    ("app/models/note.rb", NOTE),
    ("app/models/ledger.rb", LEDGER),
    ("app/models/mixins_probe.rb", PROBE),
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries.
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect()
}

fn at(text: &str, needle: &str) -> usize {
    text.find(needle).unwrap_or_else(|| panic!("missing {needle:?} in:\n{text}"))
}

/// The compiler side: the extend is written above the include its hook
/// needs it for, the carrier's own include reaches the class side, and
/// the reflection the hook mutates is the compiled association's.
#[test]
#[ignore = "fork spec: postgresql"]
fn mixins_keep_source_order_and_reflections_cover_compiled_associations() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| std::fs::read_to_string(emitted.join(path)).unwrap();

    let account = read("app/models/account.rb");
    let extend = at(&account, "  extend SensitiveFlags\n");
    assert!(extend < at(&account, "  include PasswordProtection\n"), "{account}");
    assert_eq!(account.matches("extend SensitiveFlags").count(), 1, "{account}");
    // The reflection the HasNotes hook reads is defined above the include.
    let reflect = at(&account, "  def self.reflect_on_association(");
    assert!(reflect < at(&account, "  include HasNotes\n"), "{account}");
    assert!(account.contains("HasManyReflection.new(:notes"), "{account}");

    // The carrier's own include reaches the class side (its methods are
    // copied as `def self.*`; what it includes is extended), above the
    // class macro that checks for it.
    let ledger = read("app/models/ledger.rb");
    let include = at(&ledger, "  include ParamFilters\n");
    let carried = at(&ledger, "  extend FilterConditions\n");
    assert!(include < carried && carried < at(&ledger, "  param_filters :since_id_filter\n"), "{ledger}");
    assert!(ledger.contains("require_relative \"filter_conditions\""), "{ledger}");

    // A polymorphic belongs_to answers its own reflection.
    let note = read("app/models/note.rb");
    assert!(note.contains("BelongsToReflection.new(:owner, \"owner_id\", nil, \"owner_type\", nil, { polymorphic: true }, ::Note)"), "{note}");
}

/// The model files natively and emitted, same PostgreSQL.
#[test]
#[ignore = "fork spec: postgresql"]
fn mixins_and_reflections_load_and_match_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let Some((native, emitted)) = pg_probe::native_and_emitted(
        "class-body-mixins",
        &FILES,
        &emitted,
        "rh_spec_class_body_mixins",
        "MixinsProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted, native, "emitted");
}

/// An option the compiler cannot copy as a literal is named on the
/// reflection, so reading `options` raises with the names instead of
/// answering a hash without them.
#[test]
#[ignore = "fork spec: postgresql"]
fn non_literal_association_options_are_named_not_dropped() {
    const SHELF: &str = r#"class Shelf < ApplicationRecord
  OWNER = :owner

  has_many :notes, as: OWNER, foreign_key: "owner_id" do
    def first_body
      first&.body
    end
  end
end
"#;
    let structure = format!(
        "{STRUCTURE}CREATE TABLE shelves (\n    id bigserial NOT NULL,\n    CONSTRAINT shelves_pkey PRIMARY KEY (id)\n);\n"
    );
    let (emitted, errors) = overlay()
        .write("db/structure.sql", &structure)
        .write("app/models/shelf.rb", SHELF)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shelf = std::fs::read_to_string(emitted.join("app/models/shelf.rb")).unwrap();
    // A block with a `def` is not read back, so it is named too.
    assert!(shelf.contains(r#"unsupported: ["`as:` (not a literal)", "`extend:` (the declaration's block)"])"#), "{shelf}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_mixin_only_association_block_is_carried_as_the_extend_option() {
    const SHELF: &str = r#"class Shelf < ApplicationRecord
  module Ordered
    def ordered_marker
      :ordered
    end
  end

  has_many :notes, foreign_key: "owner_id", as: :owner do
    include Ordered
  end
  has_many :other_notes, class_name: "Note", foreign_key: "owner_id" do
    include Ordered
    extend Ordered
  end
  has_many :late_notes, class_name: "Note", foreign_key: "owner_id" do
    def first_body
      first&.body
    end
  end
end
"#;
    let structure = format!(
        "{STRUCTURE}CREATE TABLE shelves (\n    id bigserial NOT NULL,\n    CONSTRAINT shelves_pkey PRIMARY KEY (id)\n);\n"
    );
    let (emitted, errors) = overlay()
        .write("db/structure.sql", &structure)
        .write("app/models/shelf.rb", SHELF)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shelf = std::fs::read_to_string(emitted.join("app/models/shelf.rb")).unwrap();
    assert!(shelf.contains("as: :owner, extend: [(Module.new do include Ordered end)] }, ::Shelf)"), "{shelf}");
    let other = &shelf[shelf.find("other_notes: ::ActiveRecord").unwrap()..];
    let other = &other[..other.find("::Shelf").unwrap()];
    assert!(other.contains("extend: [(Module.new do") && other.contains("extend Ordered"), "{shelf}");
    assert!(shelf.contains("{ class_name: \"Note\", foreign_key: \"owner_id\" }, ::Shelf, unsupported: [\"`extend:` (the declaration's block)\"])"), "{shelf}");
}

/// Callback options (`before_add: proc { ... }`, method-name symbols) and a
/// `class_name:` interpolating `self` are carried on the reflection as
/// source, as Rails keeps them in `options`; a callable that reads a local
/// of the class body around it cannot be rebuilt there and stays named.
#[test]
#[ignore = "fork spec: postgresql"]
fn callable_association_options_are_carried_on_the_reflection() {
    const SHELF: &str = r##"class Shelf < ApplicationRecord
  limit = 3

  has_many :notes, foreign_key: "owner_id", as: :owner,
    before_add: proc { |_, note| note.body = "x" },
    after_add: :note_added,
    after_remove: [:note_removed, :note_gone],
    before_remove: ->(shelf, note) { [shelf, note].compact.size }
  has_many :other_notes, class_name: "#{self}Note", foreign_key: "owner_id",
    before_add: proc { |_, note| note.body = limit.to_s }

  def note_added(_note); end
  def note_removed(_note); end
  def note_gone(_note); end
end
"##;
    let structure = format!(
        "{STRUCTURE}CREATE TABLE shelves (\n    id bigserial NOT NULL,\n    CONSTRAINT shelves_pkey PRIMARY KEY (id)\n);\n"
    );
    let (emitted, errors) = overlay()
        .write("db/structure.sql", &structure)
        .write("app/models/shelf.rb", SHELF)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shelf = std::fs::read_to_string(emitted.join("app/models/shelf.rb")).unwrap();
    let carried = r#"{ foreign_key: "owner_id", as: :owner, before_add: proc { |_, note| note.body = "x" }, after_add: :note_added, after_remove: [:note_removed, :note_gone], before_remove: ->(shelf, note) { [shelf, note].compact.size } }, ::Shelf)"#;
    assert!(shelf.contains(carried), "{shelf}");
    let other = r##"{ class_name: "#{self}Note", foreign_key: "owner_id" }, ::Shelf, unsupported: ["`before_add:` (not a literal)"])"##;
    assert!(shelf.contains(other), "{shelf}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_constant_extend_option_is_carried_on_the_reflection() {
    const SHELF: &str = r#"class Shelf < ApplicationRecord
  module Ordered
  end
  module Tagged
  end

  has_many :notes, foreign_key: "owner_id", as: :owner, extend: Ordered
  has_many :other_notes, class_name: "Note", foreign_key: "owner_id", extend: [Ordered, Tagged]
  has_many :late_notes, class_name: "Note", foreign_key: "owner_id", extend: Ordered do
    include Tagged
  end
end
"#;
    let structure = format!(
        "{STRUCTURE}CREATE TABLE shelves (\n    id bigserial NOT NULL,\n    CONSTRAINT shelves_pkey PRIMARY KEY (id)\n);\n"
    );
    let (emitted, errors) = overlay()
        .write("db/structure.sql", &structure)
        .write("app/models/shelf.rb", SHELF)
        .emit(roundhouse::project::BuildTarget::Ruby);
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let shelf = std::fs::read_to_string(emitted.join("app/models/shelf.rb")).unwrap();
    assert!(shelf.contains("as: :owner, extend: Ordered }, ::Shelf)"), "{shelf}");
    assert!(shelf.contains("foreign_key: \"owner_id\", extend: [Ordered, Tagged] }, ::Shelf)"), "{shelf}");
    assert!(shelf.contains("extend: [*Ordered, (Module.new do include Tagged end)] }, ::Shelf)"), "{shelf}");
    assert!(!shelf.contains("unsupported: [\"`extend:`"), "{shelf}");
}
