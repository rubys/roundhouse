//! PostgreSQL: attribute macros, on the Ruby target against native Rails.
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
    token_wrapped character varying,
    note_wrapped character varying,
    CONSTRAINT accounts_pkey PRIMARY KEY (id)
);
"#;

/// The ActiveModel::Validations port translates through I18n, which the
/// lockfile names.
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

fn database_yml(database: &str) -> String {
    format!("default: &default\n  adapter: postgresql\n  encoding: unicode\n\ntest:\n  <<: *default\n  database: {database}\n")
}

struct Case {
    name: &'static str,
    database: &'static str,
    probe: &'static str,
    app: &'static [(&'static str, &'static str)],
    expected: &'static [&'static str],
}

impl Case {
    fn files(&self) -> Vec<(String, String)> {
        let mut files = vec![
            ("roundhouse.yml".to_string(), "".to_string()),
            ("Gemfile.lock".to_string(), LOCK.to_string()),
            ("config/database.yml".to_string(), database_yml(self.database)),
            ("config/routes.rb".to_string(), "Rails.application.routes.draw do\nend\n".to_string()),
            ("db/structure.sql".to_string(), STRUCTURE.to_string()),
            ("app/models/application_record.rb".to_string(), APPLICATION_RECORD.to_string()),
        ];
        files.extend(self.app.iter().map(|(p, t)| (p.to_string(), t.to_string())));
        files
    }

    fn overlay(&self) -> emit_and_run::Overlay {
        self.files().iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
    }

    fn emit(&self) -> (emit_and_run::Emitted, Vec<String>) {
        self.overlay().emit(roundhouse::project::BuildTarget::Ruby)
    }

    /// The probe on native Rails and emitted, on the same PostgreSQL.
    fn check(&self) {
        let (emitted, errors) = self.emit();
        let files = self.files();
        let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), t.as_str())).collect();
        let loads: Vec<&str> = std::iter::once("app/models/application_record")
            .chain(self.app.iter().map(|(p, _)| p.trim_end_matches(".rb")))
            .collect();
        let Some((native, emitted_lines)) = pg_probe::native_and_emitted(self.name, &files, &emitted, self.database, self.probe, &loads) else { return };
        assert_eq!(native, self.expected, "native");
        assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
        assert_eq!(emitted_lines, native, "emitted");
    }
}

/// `check`'s errors other than the PostgreSQL adapter notice every
/// PostgreSQL probe app carries, and the notice for `include
/// ActiveModel::Attributes` (no port: the attribute declarations of its
/// includer are lowered by `lower::sti_subclass_attributes`).
fn real_errors(errors: &[String]) -> Vec<&String> {
    errors
        .iter()
        .filter(|e| !e.contains("database_adapter not supported") && !e.contains("includes unresolved ActiveModel::Attributes"))
        .collect()
}

// ---- 1. a macro that computes the attribute name and installs its type

const WRAPPED_TYPE: &str = r##"module Wrapping
  class WrappedType < ActiveRecord::Type::String
    def initialize(key, owner)
      super()
      @key = key
      @owner = owner
    end

    def serialize(value)
      value.nil? ? nil : "#{@key}:#{value}"
    end

    def deserialize(value)
      value.nil? ? nil : value.delete_prefix("#{@key}:")
    end
  end
end
"##;

const WRAPPING: &str = r##"module Wrapping
  def wrap_attribute(*names, key: "k", legacy: false)
    raise ArgumentError, "legacy is not supported" if legacy

    target = wrapped_methods

    names.each do |name|
      column = "#{name}_wrapped".to_sym
      type = WrappedType.new(key, self.class.name)

      attribute(column, type)

      target.send(:define_method, name) do
        self[column]
      end

      target.send(:define_method, "#{name}=") do |value|
        self[column] = value
      end
    end
  end

  private

  def wrapped_methods
    @wrapped_methods ||= Module.new.tap do |mod|
      include(mod)
    end
  end
end
"##;

const ACCOUNT: &str = r##"class Account < ApplicationRecord
  extend Wrapping

  KEYS = ["v1"].freeze
  wrap_attribute(:token, :note, key: KEYS.first)
end
"##;

const WRAPPED_PROBE: &str = r##"class WrappedProbe
  def self.lines
    out = []
    a = Account.create!(name: "n", token: "abc")
    out << "token: #{a.token.inspect}"
    out << "column: #{a.token_wrapped.inspect}"
    b = Account.find(a.id)
    out << "reloaded: #{b.token.inspect} #{b.note.inspect}"
    out << "stored: #{Account.connection.select_value("select token_wrapped from accounts").inspect}"
    b.note = "hi"
    b.save!
    out << "note: #{Account.find(a.id).note.inspect}"
    out
  end
end
"##;

const WRAPPED: Case = Case {
    name: "attribute-macros-wrapped",
    database: "rh_spec_attribute_macros_wrapped",
    probe: "WrappedProbe",
    app: &[
        ("app/models/concerns/wrapping.rb", WRAPPING),
        ("app/models/concerns/wrapping/wrapped_type.rb", WRAPPED_TYPE),
        ("app/models/account.rb", ACCOUNT),
        ("app/models/wrapped_probe.rb", WRAPPED_PROBE),
    ],
    expected: &[
        r#"token: "abc""#,
        r#"column: "abc""#,
        r#"reloaded: "abc" nil"#,
        r#"stored: "v1:abc""#,
        r#"note: "hi""#,
    ],
};

#[test]
fn a_macro_computing_the_attribute_name_compiles() {
    let (_emitted, errors) = WRAPPED.emit();
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_macro_computing_the_attribute_name_installs_its_type_and_accessors() {
    WRAPPED.check();
}

// ---- 2. attribute :x, :type in a subclass of a tableless class

const TASK_BASE: &str = r##"class TaskBase
  include ActiveModel::Attributes
  include ActiveModel::Validations
end
"##;

const TASK: &str = r##"class Task < TaskBase
  attribute :shop_id, :integer
  attribute :cash, :decimal
  attribute :note, :string, default: "none"

  validates :shop_id, presence: true

  def collection
    [shop_id, cash, note]
  end
end
"##;

const TASK_PROBE: &str = r##"class TaskProbe
  def self.lines
    out = []
    t = Task.new
    out << "blank: #{t.valid?} #{t.errors.full_messages.inspect} #{t.note.inspect}"
    t.shop_id = "42"
    t.cash = "1.5"
    out << "assigned: #{t.valid?} #{t.collection.inspect}"
    out
  end
end
"##;

const TASKS: Case = Case {
    name: "attribute-macros-tasks",
    database: "rh_spec_attribute_macros_tasks",
    probe: "TaskProbe",
    app: &[
        ("app/services/task_base.rb", TASK_BASE),
        ("app/services/task.rb", TASK),
        ("app/models/task_probe.rb", TASK_PROBE),
    ],
    expected: &[
        r#"blank: false ["Shop can't be blank"] "none""#,
        r#"assigned: true [42, 0.15e1, "none"]"#,
    ],
};

#[test]
fn a_tableless_subclass_with_attributes_compiles() {
    let (_emitted, errors) = TASKS.emit();
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_tableless_subclass_declares_its_attributes() {
    TASKS.check();
}

// ---- 3. a Concern's included { class_attribute } on an anonymous class

const FLAGGED: &str = r##"module Flagged
  extend ActiveSupport::Concern

  included do
    class_attribute :generated, default: false
    class_attribute :origin, default: nil
  end
end
"##;

const BASE_KIND: &str = r##"class BaseKind
  class_attribute :ignored, default: false
end
"##;

const FLAG_PROBE: &str = r##"class FlagProbe
  def self.build(origin)
    Class.new(BaseKind) do
      include Flagged
      self.ignored = true
      klass = self #: as untyped
      klass.generated = true
      klass.origin = origin
    end
  end

  def self.lines
    out = []
    k = build("e1")
    out << "anon: #{k.generated} #{k.origin.inspect} #{k.ignored}"
    sub = Class.new(k)
    out << "sub: #{sub.generated} #{sub.origin.inspect}"
    sub.origin = "e2"
    out << "sub set: #{sub.origin.inspect} #{k.origin.inspect}"
    out << "base: #{BaseKind.respond_to?(:generated)}"
    out << "again: #{build("e3").origin.inspect} #{k.origin.inspect}"
    out
  end
end
"##;

const FLAGS: Case = Case {
    name: "attribute-macros-flags",
    database: "rh_spec_attribute_macros_flags",
    probe: "FlagProbe",
    app: &[
        ("app/models/concerns/flagged.rb", FLAGGED),
        ("app/models/base_kind.rb", BASE_KIND),
        ("app/models/flag_probe.rb", FLAG_PROBE),
    ],
    expected: &[
        r#"anon: true "e1" true"#,
        r#"sub: true "e1""#,
        r#"sub set: "e2" "e1""#,
        "base: false",
        r#"again: "e3" "e1""#,
    ],
};

#[test]
#[ignore = "fork spec: postgresql"]
fn a_concerns_included_class_attributes_are_inlined_into_an_anonymous_includer() {
    let (emitted, errors) = FLAGS.emit();
    assert!(real_errors(&errors).is_empty(), "{}", errors.join("\n"));
    let probe = std::fs::read_to_string(emitted.join("app/models/flag_probe.rb")).expect("flag_probe.rb");
    assert!(probe.contains("def self.generated=(value)") && probe.contains("@__class_attr_generated = false"), "{probe}");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn a_concerns_included_class_attributes_apply_to_an_anonymous_includer() {
    FLAGS.check();
}
