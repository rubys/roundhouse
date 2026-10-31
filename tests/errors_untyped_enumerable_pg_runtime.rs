//! PostgreSQL: errors untyped enumerable, on the Ruby target against native Rails.
//!
//! Ported from the Shopify/roundhouse-1 fork's tests. A test the compiler
//! does not pass yet is ignored as `fork spec: postgresql`. A probe needs
//! ROUNDHOUSE_PG_TEST_URL and the pg gem (tests/support/pg_probe.rs).
#[path = "support/emit_and_run.rs"]
mod emit_and_run;
#[path = "support/pg_probe.rs"]
mod pg_probe;

use roundhouse::project::BuildTarget;

const STRUCTURE: &str = r#"CREATE TABLE posts (
    id bigserial NOT NULL,
    title character varying,
    author_id bigint,
    CONSTRAINT posts_pkey PRIMARY KEY (id)
);
"#;

const APPLICATION_RECORD: &str = "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n";

const POST: &str = r#"class Post < ApplicationRecord
  validates :title, presence: true
  validate :checks

  def checks
    errors.add(:base, "no shouting") if title == "LOUD"
    errors.add(:author_id, "must be set") if title == "LOUD"
  end
end
"#;

const HOLDER: &str = r#"class ErrorHolder
  def initialize(errors)
    @errors = errors
  end

  def active_model_errors
    instance_variable_get(:@errors)
  end
end
"#;

const PROBE: &str = r##"class UntypedProbe
  def self.show(out, label)
    value = begin
      yield
    rescue StandardError => e
      [e.class.name, e.message]
    end
    out << "#{label}: #{value.inspect}"
  end

  def self.holder(title)
    post = Post.new(title: title)
    post.valid?
    # attribute port and records each push's attribute.
    post.errors.details
    ErrorHolder.new(post.errors)
  end

  def self.lines
    out = []
    show(out, "collect") do
      holder("LOUD").active_model_errors.collect { |error| [error.attribute, error.type] }
    end
    show(out, "map") do
      holder("").active_model_errors.map { |error| [error.attribute, error.type, error.message] }
    end
    show(out, "map symbol") { holder("LOUD").active_model_errors.map(&:attribute) }
    show(out, "flat_map") { holder("LOUD").active_model_errors.flat_map { |error| [error.attribute, error.full_message] } }
    show(out, "select") { holder("LOUD").active_model_errors.select { |error| error.attribute == :base }.map(&:full_message) }
    show(out, "reject") { holder("LOUD").active_model_errors.reject { |error| error.attribute == :base }.map(&:attribute) }
    show(out, "filter_map") { holder("LOUD").active_model_errors.filter_map { |error| error.attribute if error.type != :blank } }
    show(out, "find") { holder("LOUD").active_model_errors.find { |error| error.attribute == :author_id }.message }
    show(out, "first") { holder("LOUD").active_model_errors.first.attribute }
    show(out, "group_by") { holder("LOUD").active_model_errors.group_by(&:attribute).transform_values(&:size) }
    show(out, "each_with_object") do
      holder("LOUD").active_model_errors.each_with_object({}) { |error, seen| seen[error.attribute] = error.message }
    end
    show(out, "sort_by") { holder("LOUD").active_model_errors.sort_by { |error| error.attribute.to_s }.map(&:attribute) }
    show(out, "count with a block") { holder("LOUD").active_model_errors.count { |error| error.attribute == :base } }
    show(out, "any? with a block") { holder("LOUD").active_model_errors.any? { |error| error.attribute == :base } }
    show(out, "none? with a block") { holder("LOUD").active_model_errors.none? { |error| error.attribute == :title } }
    show(out, "blockless queries") do
      errors = holder("LOUD").active_model_errors
      [errors.count, errors.size, errors.any?, errors.empty?, errors.full_messages, errors.to_a]
    end
    show(out, "no errors") { holder("fine").active_model_errors.map(&:attribute) }
    out
  end
end
"##;

const DATABASE_YML: &str = "test:\n  adapter: postgresql\n  encoding: unicode\n  database: rh_spec_errors_untyped\n";

const FILES: [(&str, &str); 8] = [
    ("roundhouse.yml", ""),
    ("config/database.yml", DATABASE_YML),
    ("config/routes.rb", "Rails.application.routes.draw do\nend\n"),
    ("db/structure.sql", STRUCTURE),
    ("app/models/application_record.rb", APPLICATION_RECORD),
    ("app/models/post.rb", POST),
    ("app/models/error_holder.rb", HOLDER),
    ("app/models/untyped_probe.rb", PROBE),
];

const LOADS: [&str; 4] =
    ["app/models/application_record", "app/models/post", "app/models/error_holder", "app/models/untyped_probe"];

const EXPECTED: &[&str] = &[
    r#"collect: [[:base, "no shouting"], [:author_id, "must be set"]]"#,
    r#"map: [[:title, :blank, "can't be blank"]]"#,
    r#"map symbol: [:base, :author_id]"#,
    r#"flat_map: [:base, "no shouting", :author_id, "Author must be set"]"#,
    r#"select: ["no shouting"]"#,
    r#"reject: [:author_id]"#,
    r#"filter_map: [:base, :author_id]"#,
    r#"find: "must be set""#,
    r#"first: :base"#,
    r#"group_by: {base: 1, author_id: 1}"#,
    r#"each_with_object: {base: "no shouting", author_id: "must be set"}"#,
    r#"sort_by: [:author_id, :base]"#,
    r#"count with a block: 1"#,
    r#"any? with a block: true"#,
    r#"none? with a block: true"#,
    r#"blockless queries: [2, 2, true, false, ["no shouting", "Author must be set"], ["no shouting", "Author must be set"]]"#,
    r#"no errors: []"#,
];

#[test]
#[ignore = "fork spec: postgresql"]
fn untyped_errors_enumerate_error_objects_on_postgresql() {
    let overlay = FILES.iter().fold(emit_and_run::empty_app(), |app, (path, text)| app.write(path, text));
    let (emitted, errors) = overlay.emit(BuildTarget::Ruby);
    let errors: Vec<_> = errors.iter().filter(|e| !e.contains("database_adapter not supported")).collect();
    assert!(errors.is_empty(), "{errors:#?}");
    let Some((native, emitted_lines)) = pg_probe::native_and_emitted("errors-untyped", &FILES, &emitted, "rh_spec_errors_untyped", "UntypedProbe", &LOADS) else { return };
    assert_eq!(native, EXPECTED, "native");
    assert_eq!(emitted_lines, native, "emitted");
}
