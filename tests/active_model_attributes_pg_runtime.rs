//! PostgreSQL: active model attributes, on the Ruby target against native Rails.
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
  database: rh_spec_model_attributes
"#;

const STRUCTURE: &str = "CREATE TABLE units (\n    id bigserial NOT NULL,\n    CONSTRAINT units_pkey PRIMARY KEY (id)\n);\n";

/// The ActiveModel::Validations port translates through I18n, which the
/// lockfile names.
const LOCK: &str = "GEM\n  remote: https://rubygems.org/\n  specs:\n    concurrent-ruby (1.3.5)\n    i18n (1.15.2)\n      concurrent-ruby (~> 1.0)\n\nDEPENDENCIES\n  i18n\n";

const TASK_BASE: &str = r#"class TaskBase
  include ActiveModel::Attributes
  include ActiveModel::AttributeAssignment
  include ActiveModel::Validations

  class_attribute :strategy, default: nil
end
"#;
const GCS_COLLECTION: &str = r#"class TaskBase
  module GcsCollection
    extend ActiveSupport::Concern

    class_methods do
      def gcs_collection(*columns, filename: nil, required: true)
        self.strategy = columns
        attribute(:gcs_filename, :string, default: filename)
        validates(:gcs_filename, presence: true) if required
      end
    end
  end
end
"#;
const RISK: &str = r#"class TaskBase
  module Risk
    extend ActiveSupport::Concern

    class_methods do
      def risk(tier = nil)
        @risk = tier if tier
        @risk || :standard
      end
    end
  end
end
"#;
const CSV_TASK: &str = r#"class CsvTask < TaskBase
  COLUMNS = %w(a b).freeze

  risk :high
  gcs_collection(*COLUMNS.map(&:to_sym))

  attribute(:dry_run, :boolean, default: true)
  attribute(:rows, :integer, default: 20)

  def describe
    [dry_run, rows, gcs_filename, strategy, self.class.risk]
  end
end
"#;
const PLAIN_TASK: &str = r#"class PlainTask < TaskBase
  attribute(:label, :string, default: "none")
  attribute(:count, :integer)
end
"#;
const ASSIGNED: &str = r#"class Assigned
  include ActiveModel::AttributeAssignment

  attr_accessor :name, :quantity

  def initialize(attributes)
    assign_attributes(attributes)
  end
end
"#;
const INITIALIZER: &str = r#"Rails.autoloaders.main.on_load("TaskBase") do
  TaskBase.include(TaskBase::GcsCollection)
  TaskBase.include(TaskBase::Risk)
  TaskBase.class_attribute(:unscoped, default: false, instance_writer: false)
end
"#;

const PROBE: &str = r##"class ModelAttributesProbe
  def self.lines
    task = CsvTask.new
    out = []
    out << "risk: #{CsvTask.risk} #{PlainTask.risk}"
    out << "names: #{CsvTask.attribute_names.inspect} #{PlainTask.attribute_names.inspect}"
    out << "defaults: #{task.attributes.inspect}"
    out << "plain: #{PlainTask.new.attributes.inspect}"
    out << "blank gcs_filename valid: #{task.valid?}"
    task.gcs_filename = "report.csv"
    out << "set valid: #{task.valid?} #{task.gcs_filename}"
    out << "describe: #{task.describe.inspect}"
    task.assign_attributes(rows: "7", dry_run: "0")
    out << "assigned: #{task.attributes.inspect}"
    plain = PlainTask.new
    plain.count = "3"
    out << "cast: #{plain.count.inspect} #{plain.label}"
    out << "isolated: #{CsvTask.new.rows} #{task.rows} #{PlainTask.attribute_names.include?("rows")}"
    assigned = Assigned.new(name: "pen", quantity: 2)
    out << "assigned plain: #{assigned.name.inspect} #{assigned.quantity}"
    out
  end
end
"##;

const NATIVE_BOOT: &str = r#"require "zeitwerk"
require "active_support"
require "active_support/concern"
require "active_support/core_ext/class/attribute"
require "active_model"

module Rails
  def self.autoloaders
    @autoloaders ||= Struct.new(:main).new(Zeitwerk::Loader.new)
  end
end

loader = Rails.autoloaders.main
loader.push_dir(File.join(__dir__, "app/models"))
loader.push_dir(File.join(__dir__, "app/tasks"))
loader.setup
Dir[File.join(__dir__, "config/initializers/*.rb")].sort.each { |file| load file }
"#;

const EXPECTED: [&str; 11] = [
    "risk: high standard",
    r#"names: ["gcs_filename", "dry_run", "rows"] ["label", "count"]"#,
    r#"defaults: {"gcs_filename" => nil, "dry_run" => true, "rows" => 20}"#,
    r#"plain: {"label" => "none", "count" => nil}"#,
    "blank gcs_filename valid: false",
    "set valid: true report.csv",
    r#"describe: [true, 20, "report.csv", [:a, :b], :high]"#,
    r#"assigned: {"gcs_filename" => "report.csv", "dry_run" => false, "rows" => 7}"#,
    r#"cast: 3 none"#,
    "isolated: 20 7 false",
    r#"assigned plain: "pen" 2"#,
];

const FILES: [(&str, &str); 12] = [
    ("roundhouse.yml", ""),
    ("Gemfile.lock", LOCK),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/task_base.rb", TASK_BASE),
    ("app/models/task_base/gcs_collection.rb", GCS_COLLECTION),
    ("app/models/task_base/risk.rb", RISK),
    ("app/tasks/csv_task.rb", CSV_TASK),
    ("app/tasks/plain_task.rb", PLAIN_TASK),
    ("app/models/assigned.rb", ASSIGNED),
    ("config/initializers/task_base.rb", INITIALIZER),
];

#[test]
#[ignore = "fork spec: postgresql native control"]
fn attributes_and_inherited_class_macros_match_native_rails() {
    let overlay = FILES
        .iter()
        .chain([&("app/models/model_attributes_probe.rb", PROBE)])
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let native_files: Vec<(&str, &str)> = FILES
        .iter()
        .copied()
        .chain([("app/models/model_attributes_probe.rb", PROBE), ("native_boot.rb", NATIVE_BOOT)])
        .collect();
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted(
        "model-attributes",
        &native_files,
        &emitted,
        "rh_spec_model_attributes",
        "ModelAttributesProbe",
        &["native_boot"],
    ) else { return };
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}

#[test]
#[ignore = "fork spec: postgresql"]
fn the_subclass_body_calls_are_replayed_and_the_initializer_extends_its_concerns() {
    let overlay = FILES
        .iter()
        .fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(roundhouse::project::BuildTarget::Ruby);
    let real: Vec<&String> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(real.is_empty(), "{}", errors.join("\n"));
    let read = |path: &str| {
        std::fs::read_to_string(emitted.join(path)).unwrap_or_else(|e| panic!("emitted {path}: {e}"))
    };
    let csv = read("app/models/csv_task.rb");
    for call in ["risk :high", "gcs_collection(", "attribute(:dry_run", "attribute(:rows"] {
        assert!(csv.contains(call), "the subclass body keeps {call}:\n{csv}");
    }
    let initializer = read("app/models/roundhouse_initializer_config_initializers_task_base.rb");
    for carrier in ["TaskBase.extend ::TaskBase::GcsCollection::ClassMethods", "TaskBase.extend ::TaskBase::Risk::ClassMethods"] {
        assert!(initializer.contains(carrier), "the initializer's include extends its concern: {carrier}\n{initializer}");
    }
    assert!(emitted.join("runtime/active_model_attributes.rb").exists(), "the Attributes port is carried");
    assert!(read("app/models/task_base.rb").contains("include ActiveModel::Attributes"), "the base keeps its include");
}
