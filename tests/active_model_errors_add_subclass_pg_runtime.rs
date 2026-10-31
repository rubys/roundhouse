//! PostgreSQL: active model errors add subclass, on the Ruby target against native Rails.
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
  database: rh_spec_errors_add_subclass
"#;

const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const APPLICATION_RECORD: &str = r#"class ApplicationRecord < ActiveRecord::Base
  self.abstract_class = true
end
"#;

const BASE: &str = r#"class TaskBase
  include ActiveModel::Attributes
  include ActiveModel::AttributeAssignment
  include ActiveModel::Validations

  def initialize(attrs = {})
    super()
    assign_attributes(attrs)
  end

  attribute :shop_ids, :string
  attribute :gcs_filename, :string
  attribute :kinds, :string
end
"#;

const TASK: &str = r#"class SemanticTask < TaskBase
  GRANTABLE = %w[a b].freeze

  validate :mutually_exclusive
  validate :kinds_grantable
  validates :shop_ids, length: { maximum: 5 }

  private

  def mutually_exclusive
    selected = [gcs_filename, shop_ids].count { |x| x.present? }
    if selected > 1
      errors.add(:shop_ids, :provide_only_one_input, message: "cannot be used if gcs_filename is set")
      errors.add(:gcs_filename, :provide_only_one_input, message: "cannot be used if shop_ids is set")
    end
  end

  def kinds_grantable
    listed = kinds.to_s.split(",").map(&:strip).reject(&:empty?)
    if listed.empty?
      errors.add(:kinds, "must list at least one of: #{GRANTABLE.join(', ')}")
      return
    end
    bad = listed - GRANTABLE
    errors.add(:kinds, "lists kinds this task cannot grant: #{bad.join(', ')}") unless bad.empty?
  end
end
"#;

const PROBE: &str = r##"class ErrorsAddSubclassProbe
  def self.lines
    out = []
    ok = SemanticTask.new(kinds: "a, b", shop_ids: "1")
    out << "ok: #{ok.valid?} #{ok.errors.full_messages.inspect}"
    both = SemanticTask.new(kinds: "a", shop_ids: "1", gcs_filename: "f.csv")
    out << "both: #{both.valid?} #{both.errors.full_messages.inspect}"
    none = SemanticTask.new
    out << "none: #{none.valid?} #{none.errors.full_messages.inspect}"
    bad = SemanticTask.new(kinds: "a, z")
    out << "bad: #{bad.invalid?} #{bad.errors.full_messages.inspect} #{bad.errors[:kinds].inspect}"
    long = SemanticTask.new(kinds: "a", shop_ids: "123456")
    out << "long: #{long.valid?} #{long.errors.full_messages.inspect}"
    out
  end
end
"##;

const EXPECTED: [&str; 5] = [
    r#"ok: true []"#,
    r#"both: false ["Shop ids cannot be used if gcs_filename is set", "Gcs filename cannot be used if shop_ids is set"]"#,
    r#"none: false ["Kinds must list at least one of: a, b"]"#,
    r#"bad: true ["Kinds lists kinds this task cannot grant: z"] ["lists kinds this task cannot grant: z"]"#,
    r#"long: false ["Shop ids is too long (maximum is 5 characters)"]"#,
];

const FILES: [(&str, &str); 9] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", "\n"),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/task_base.rb", BASE),
    ("app/models/semantic_task.rb", TASK),
    ("app/models/errors_add_subclass_probe.rb", PROBE),
];

const LOADS: [&str; 4] = [
    "app/models/application_record.rb",
    "app/models/task_base.rb",
    "app/models/semantic_task.rb",
    "app/models/errors_add_subclass_probe.rb",
];

fn overlay() -> emit_and_run::Overlay {
    FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text))
}

#[test]
#[ignore = "fork spec: postgresql"]
fn errors_add_in_a_subclass_matches_native_rails_on_postgresql() {
    let (emitted, errors) = overlay().emit(roundhouse::project::BuildTarget::Ruby);
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "errors-add-subclass",
        &FILES,
        &emitted,
        "rh_spec_errors_add_subclass",
        "ErrorsAddSubclassProbe",
        &LOADS,
    ) else { return };
    assert_eq!(native, EXPECTED, "native");
    let real: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    assert_eq!(emitted_lines, native, "emitted");
}
