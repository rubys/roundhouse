//! A generated-column construct is supported only when the emitted model
//! runs cleanly against real SQLite DDL and reads the database-computed
//! value. Rails 8.1's SQLite oracle keeps update values stale until explicit
//! reload, while create's RETURNING hydrates the generated field before hooks.

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

fn app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            include_str!("support/generated_columns_schema.rb"),
        )
        .write(
            "app/models/person.rb",
            include_str!("support/generated_columns_person.rb"),
        )
        .write(
            "app/models/virtual_person.rb",
            include_str!("support/generated_columns_virtual_person.rb"),
        )
        .write(
            "app/models/constant_person.rb",
            include_str!("support/generated_columns_constant_person.rb"),
        )
}

fn alternate_key_app() -> emit_and_run::Overlay {
    emit_and_run::empty_app()
        .write(
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        )
        .write(
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        )
        .write(
            "db/schema.rb",
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "uuid_people", id: :uuid, force: :cascade do |t|
    t.string "first_name"
    t.virtual "label", type: :string, as: "first_name || '-uuid'", stored: true
  end

  create_table "string_key_people", primary_key: "person_key", id: :string, force: :cascade do |t|
    t.string "first_name"
    t.virtual "label", type: :string, as: "first_name || '-string'", stored: true
  end

  create_table "rowid_people", id: false, force: :cascade do |t|
    t.string "first_name"
    t.virtual "label", type: :string, as: "first_name || '-rowid'", stored: true
  end
end
"#,
        )
        .write("app/models/uuid_person.rb", "class UuidPerson < ApplicationRecord\nend\n")
        .write(
            "app/models/string_key_person.rb",
            "class StringKeyPerson < ApplicationRecord\n  self.primary_key = \"person_key\"\nend\n",
        )
        .write(
            "app/models/rowid_person.rb",
            "class RowidPerson < ApplicationRecord\nend\n",
        )
}

#[test]
fn emitted_cruby_models_read_and_persist_generated_columns_like_rails() {
    let run = app().run_ruby(include_str!("support/generated_columns_contract.rb"));
    run.assert_passes();
    assert!(
        run.stdout
            .contains("generated column create/update/reload contract passed")
    );
}

#[test]
fn generated_insert_does_not_retry_after_a_hydration_select_failure() {
    let run = app().run_ruby(
        r##"
# Inject a failure only for the former post-insert generated-value query.
# This is a harness-side fault: the emitted application itself is unchanged.
module FailGeneratedHydrationSelect
  def prepare(sql)
    normalized = sql.gsub(/["`]/, "").downcase.gsub(/\s+/, " ")
    if normalized.start_with?("select ") &&
       normalized.include?("from people where id =") &&
       normalized.include?("display_name") &&
       normalized.include?("normalized_first_name")
      raise "injected generated-column hydration SELECT failure"
    end
    super
  end
end
Db.singleton_class.prepend(FailGeneratedHydrationSelect)

person = Person.new(first_name: "Retry", last_name: "Control")
first_failure = nil
retry_failure = nil
sqls = Db.capture_sql do
  begin
    person.save!
  rescue StandardError => failure
    first_failure = failure
    begin
      person.save!
    rescue StandardError => retry_error
      retry_failure = retry_error
    end
  end
end
raise "the generated insert reached the injected follow-up SELECT: #{first_failure.inspect}" unless first_failure.nil?
raise "the retry unexpectedly failed: #{retry_failure.inspect}" unless retry_failure.nil?
raise "the returned key was not retained" if person.id.nil? || person.id <= 0
raise "the generated value was not returned" unless person.display_name == "Retry Control"
raise "the failed-hydration retry duplicated the insert" unless Person.count == 1
inserts = sqls.select { |sql| sql.include?("INSERT INTO") && sql.include?("people") }
raise "expected one generated INSERT attempt: #{sqls.inspect}" unless inserts.length == 1
raise "generated INSERT did not use RETURNING: #{inserts.first}" unless inserts.first.include?("RETURNING")
puts "generated insert avoids retryable hydration SELECT"
"##,
    );
    run.assert_passes();
    assert!(
        run.stdout
            .contains("generated insert avoids retryable hydration SELECT")
    );
}

#[test]
fn generated_returning_hydrates_values_when_after_insert_trigger_deletes_row() {
    let run = app().run_ruby(
        r##"
Db.exec(<<~SQL)
  CREATE TRIGGER delete_people_after_insert
  AFTER INSERT ON people
  BEGIN
    DELETE FROM people WHERE id = NEW.id;
  END
SQL

person = nil
sqls = Db.capture_sql do
  person = Person.create!(first_name: "Returned", last_name: "Before Delete")
end
raise "RETURNING did not retain the inserted key" if person.id.nil? || person.id <= 0
raise "RETURNING did not hydrate the generated value" unless person.display_name == "Returned Before Delete"
raise "after_create did not see the RETURNING value" unless person.generated_after_create == "Returned Before Delete"
raise "AFTER INSERT trigger should have removed the persisted row" unless Person.count == 0
insert = sqls.find { |sql| sql.include?("INSERT INTO") && sql.include?("people") }
raise "generated insert did not use RETURNING: #{sqls.inspect}" unless insert && insert.include?("RETURNING")
raise "generated insert issued a follow-up SELECT: #{sqls.inspect}" if sqls.any? { |sql| sql.start_with?("SELECT") && sql.include?("people") && sql.include?("display_name") }
puts "trigger-delete generated value came from RETURNING"
"##,
    );
    run.assert_passes();
    assert!(
        run.stdout
            .contains("trigger-delete generated value came from RETURNING")
    );
}

#[test]
fn generated_returning_preserves_model_keys_and_adapter_rowid() {
    let run = alternate_key_app().run_ruby(
        r##"
require "securerandom"

uuid = nil
uuid_sql = Db.capture_sql { uuid = UuidPerson.create!(first_name: "uuid") }
raise "UUID key was not returned" unless uuid.id.match?(/\A[0-9a-f-]{36}\z/i)
raise "UUID generated value was not hydrated" unless uuid.label == "uuid-uuid"
uuid_insert = uuid_sql.find { |sql| sql.include?("INSERT INTO") && sql.include?("uuid_people") }
raise "UUID INSERT did not return its declared key: #{uuid_sql.inspect}" unless uuid_insert && uuid_insert.include?("RETURNING") && uuid_insert.split("RETURNING").last.include?("id")

string_key = nil
string_sql = Db.capture_sql do
  string_key = StringKeyPerson.create!(person_key: "person-key", first_name: "string")
end
raise "string primary key was not returned" unless string_key.id == "person-key"
raise "string-key generated value was not hydrated" unless string_key.label == "string-string"
string_insert = string_sql.find { |sql| sql.include?("INSERT INTO") && sql.include?("string_key_people") }
raise "string-key INSERT did not return its declared key: #{string_sql.inspect}" unless string_insert && string_insert.include?("RETURNING") && string_insert.split("RETURNING").last.include?("person_key")

# No-primary-key models do not implement the general save lifecycle's id=;
# call the synthesized adapter directly to cover its SQLite rowid fallback.
rowid = RowidPerson.new(first_name: "implicit")
rowid_key = nil
rowid_sql = Db.capture_sql { rowid_key = rowid._adapter_insert }
raise "implicit rowid was not returned" unless rowid_key.is_a?(Integer) && rowid_key > 0
raise "implicit-rowid generated value was not hydrated" unless rowid.label == "implicit-rowid"
rowid_insert = rowid_sql.find { |sql| sql.include?("INSERT INTO") && sql.include?("rowid_people") }
raise "implicit-rowid INSERT did not return rowid: #{rowid_sql.inspect}" unless rowid_insert && rowid_insert.include?("RETURNING") && rowid_insert.split("RETURNING").last.downcase.include?("rowid")
puts "generated RETURNING preserves UUID/string model keys and adapter rowid"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("generated RETURNING preserves UUID/string model keys and adapter rowid"));
}

#[test]
fn generated_returning_read_error_finalizes_handle_without_partial_cache_updates() {
    let run = app().run_ruby(
        r##"
module FailSecondGeneratedReturningTextRead
  def exec_returning(sql)
    handle = super
    if sql.include?("INSERT INTO") && sql.include?("people")
      @rh_test_generated_handle = handle
      @rh_test_generated_reads = []
      @rh_test_generated_finalizes = 0
    end
    handle
  end

  def column_text_opt(handle, index)
    if handle.equal?(@rh_test_generated_handle) && [1, 2].include?(index)
      @rh_test_generated_reads << index
      raise "injected second generated-column read failure" if index == 2
    end
    super
  end

  def finalize(handle)
    @rh_test_generated_finalizes += 1 if handle.equal?(@rh_test_generated_handle)
    super
  end
end
Db.singleton_class.prepend(FailSecondGeneratedReturningTextRead)

# This failure happens while decoding an already materialized INSERT result.
# It tests staging and handle cleanup; the database write itself is not rolled back.
person = Person.new(first_name: "Decode", last_name: "Failure")
failure = nil
sqls = Db.capture_sql do
  begin
    person.save!
  rescue StandardError => error
    failure = error
  end
end
raise "the injected second value read did not fail" unless failure && failure.message.include?("injected second generated-column read failure")
raise "generated value reads were not staged in order" unless Db.instance_variable_get(:@rh_test_generated_reads) == [1, 2]
raise "the returned handle was not finalized after a read error" unless Db.instance_variable_get(:@rh_test_generated_finalizes) == 1
raise "first generated cache changed before all reads succeeded" unless person.display_name.nil?
raise "second generated cache changed before all reads succeeded" unless person.normalized_first_name.nil?
raise "the already executed INSERT should remain persisted" unless Person.count == 1
raise "the create used a follow-up SELECT: #{sqls.inspect}" if sqls.any? { |sql| sql.start_with?("SELECT") && sql.include?("people") && sql.include?("display_name") }
puts "generated RETURNING read failure finalizes without partial cache updates"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("generated RETURNING read failure finalizes without partial cache updates"));
}

#[test]
fn generated_returning_missing_row_raises_and_finalizes_handle() {
    let run = app().run_ruby(
        r##"
Db.exec(<<~SQL)
  CREATE TRIGGER ignore_people_before_insert
  BEFORE INSERT ON people
  BEGIN
    SELECT RAISE(IGNORE);
  END
SQL

module TrackMissingGeneratedReturningHandle
  def exec_returning(sql)
    handle = super
    if sql.include?("INSERT INTO") && sql.include?("people")
      @rh_test_missing_handle = handle
      @rh_test_missing_finalizes = 0
    end
    handle
  end

  def finalize(handle)
    @rh_test_missing_finalizes += 1 if handle.equal?(@rh_test_missing_handle)
    super
  end
end
Db.singleton_class.prepend(TrackMissingGeneratedReturningHandle)

person = Person.new(first_name: "Ignored", last_name: "Insert")
failure = nil
Db.capture_sql do
  begin
    person.save!
  rescue StandardError => error
    failure = error
  end
end
raise "missing RETURNING row did not produce the explicit generated-insert error" unless failure && failure.message.include?("INSERT ... RETURNING produced no row for `people`")
raise "missing-row RETURNING handle was not finalized" unless Db.instance_variable_get(:@rh_test_missing_finalizes) == 1
raise "ignored insert unexpectedly persisted" unless Person.count == 0
puts "generated RETURNING missing row raises and finalizes"
"##,
    );
    run.assert_passes();
    assert!(run.stdout.contains("generated RETURNING missing row raises and finalizes"));
}

#[test]
fn direct_bulk_writes_of_generated_keys_preserve_database_rejection() {
    app()
        .run_ruby(
            r##"
person = Person.create!(first_name: "Ada", last_name: "Lovelace")
cases = [
  ["update_all", "UPDATE", -> { Person.where(id: person.id).update_all(display_name: "forged-update-all") }],
  ["upsert_all", "INSERT", -> { Person.upsert_all([{ first_name: "Bulk", last_name: "Upsert", display_name: "forged-upsert-all" }]) }],
]
cases.each do |name, verb, operation|
  error = nil
  attempted_sql = Db.capture_sql do
    begin
      operation.call
    rescue StandardError => failure
      error = failure
    end
  end
  raise "#{name} silently accepted an explicit generated value" if error.nil?
  raise "#{name} failed for the wrong reason: #{error.class}: #{error.message}" unless error.message.downcase.include?("generated column")
  raise "#{name} did not reach SQLite with the explicit generated key: #{attempted_sql.inspect}" unless attempted_sql.any? { |sql| sql.include?(verb) && sql.include?("display_name") }
end
puts "explicit bulk generated-column writes remain rejected"
"##,
        )
        .assert_passes();
}

#[test]
fn model_insert_all_with_generated_key_is_a_located_compile_error() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let model = r#"class Person < ApplicationRecord
  def unsafe_generated_insert
    Person.insert_all([{ first_name: "Bulk", last_name: "Insert", display_name: "forged" }])
  end
end
"#;
    let (_emitted, app, diagnostics) = app()
        .write("app/models/person.rb", model)
        .emit_with_app(BuildTarget::Ruby);

    let diagnostic = diagnostics.iter().find(|diagnostic| {
        if diagnostic.severity != Severity::Error
            || !diagnostic.message.to_lowercase().contains("generated")
        {
            return false;
        }
        roundhouse::ide::source(&app, diagnostic.span.file).is_some_and(|source| {
            Path::new(&source.path).ends_with("app/models/person.rb")
                && source
                    .text
                    .lines()
                    .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                    .is_some_and(|line| line.contains("Person.insert_all"))
        })
    });
    let diagnostic = diagnostic.expect(
        "app-method insert_all of a generated value must not lower to repeated saves and pass with zero errors",
    );
    assert!(
        !diagnostic.span.is_synthetic(),
        "guard must retain a source location: {diagnostic:?}"
    );
    let source = roundhouse::ide::source(&app, diagnostic.span.file).expect("guard source");
    assert!(
        Path::new(&source.path).ends_with("app/models/person.rb"),
        "{}",
        source.path
    );
}

#[test]
fn generated_instance_writes_are_guarded_at_their_source_calls() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let model = r#"class Person < ApplicationRecord
  def explicit_generated_update
    self.update_column(:display_name, "forged")
  end

  def explicit_generated_update_with_block
    self.update_column(:display_name, "forged") { 1 }
  end

  def implicit_generated_update
    update_column(:display_name, "forged")
  end

  def dynamic_generated_update(column_name)
    update_column(column_name, "forged")
  end

  def generated_touch
    touch(:display_name)
  end

  def dynamic_generated_touch(column_name)
    touch(column_name)
  end
end
"#;
    let (_emitted, app, diagnostics) = app()
        .write("app/models/person.rb", model)
        .emit_with_app(BuildTarget::Ruby);

    let assert_guard = |path: &str, call: &str| {
        let diagnostic = diagnostics.iter().find(|diagnostic| {
            if diagnostic.severity != Severity::Error
                || !diagnostic.message.to_lowercase().contains("generated")
            {
                return false;
            }
            roundhouse::ide::source(&app, diagnostic.span.file).is_some_and(|source| {
                Path::new(&source.path).ends_with(path)
                    && source
                        .text
                        .lines()
                        .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                        .is_some_and(|line| line.contains(call))
            })
        });
        let diagnostic = diagnostic.unwrap_or_else(|| panic!("missing located generated-write guard for {path}: {call}; diagnostics: {diagnostics:?}"));
        assert!(
            !diagnostic.span.is_synthetic(),
            "guard must retain a source location: {diagnostic:?}"
        );
    };

    assert_guard("app/models/person.rb", "self.update_column(:display_name");
    assert_guard(
        "app/models/person.rb",
        "self.update_column(:display_name, \"forged\") { 1 }",
    );
    assert_guard("app/models/person.rb", "    update_column(:display_name");
    assert_guard("app/models/person.rb", "    update_column(column_name");
    assert_guard("app/models/person.rb", "    touch(:display_name");
    assert_guard("app/models/person.rb", "    touch(column_name");
}

#[test]
fn typed_controller_receiver_is_guarded_and_ordinary_update_column_is_allowed() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let controller = r#"class GeneratedColumnWritesController < ApplicationController
  def unsafe_update
    person = Person.find(1)
    person.update_column(:display_name, "forged")
  end
end
"#;
    let (_emitted, analyzed_app, diagnostics) = app()
        .write(
            "app/controllers/generated_column_writes_controller.rb",
            controller,
        )
        .emit_with_app(BuildTarget::Ruby);
    let diagnostic = diagnostics.iter().find(|diagnostic| {
        if diagnostic.severity != Severity::Error
            || !diagnostic.message.to_lowercase().contains("generated")
        {
            return false;
        }
        roundhouse::ide::source(&analyzed_app, diagnostic.span.file).is_some_and(|source| {
            Path::new(&source.path)
                .ends_with("app/controllers/generated_column_writes_controller.rb")
                && source
                    .text
                    .lines()
                    .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                    .is_some_and(|line| line.contains("person.update_column(:display_name"))
        })
    });
    let diagnostic = diagnostic
        .expect("typed model variable update must have a source-located generated-column guard");
    assert!(
        !diagnostic.span.is_synthetic(),
        "guard must retain a source location: {diagnostic:?}"
    );

    let ordinary_controller = r#"class GeneratedColumnWritesController < ApplicationController
  def safe_update
    person = Person.find(1)
    person.update_column(:first_name, "Grace")
  end
end
"#;
    let (_emitted, _app, diagnostics) = app()
        .write(
            "app/controllers/generated_column_writes_controller.rb",
            ordinary_controller,
        )
        .emit_with_app(BuildTarget::Ruby);
    assert!(
        diagnostics.is_empty(),
        "ordinary-column update_column should remain supported: {diagnostics:?}"
    );
}

#[test]
fn union_typed_receiver_is_guarded_when_any_model_branch_has_generated_columns() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let mut schema = include_str!("support/generated_columns_schema.rb").to_string();
    let schema_end = schema.rfind("end\n").expect("schema's outer end");
    schema.insert_str(
        schema_end,
        "  create_table \"plain_people\", force: :cascade do |t|\n    t.string \"name\"\n  end\n\n",
    );
    let controller = r#"class GeneratedColumnWritesController < ApplicationController
  def unsafe_union_write(use_generated, column_name)
    person = use_generated ? Person.find(1) : PlainPerson.find(1)
    person.update_column(:display_name, "forged")
  end

  def unsafe_union_dynamic_write(use_generated, column_name)
    person = use_generated ? Person.find(1) : PlainPerson.find(1)
    person.update_column(column_name, "forged")
  end
end
"#;
    let (_emitted, analyzed_app, diagnostics) = app()
        .write("db/schema.rb", &schema)
        .write(
            "app/models/plain_person.rb",
            "class PlainPerson < ApplicationRecord\nend\n",
        )
        .write(
            "app/controllers/generated_column_writes_controller.rb",
            controller,
        )
        .emit_with_app(BuildTarget::Ruby);

    for call in [
        "person.update_column(:display_name",
        "person.update_column(column_name",
    ] {
        let diagnostic = diagnostics.iter().find(|diagnostic| {
            if diagnostic.severity != Severity::Error
                || !diagnostic.message.to_lowercase().contains("generated")
            {
                return false;
            }
            roundhouse::ide::source(&analyzed_app, diagnostic.span.file).is_some_and(|source| {
                Path::new(&source.path)
                    .ends_with("app/controllers/generated_column_writes_controller.rb")
                    && source
                        .text
                        .lines()
                        .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                        .is_some_and(|line| line.contains(call))
            })
        });
        let diagnostic = diagnostic.unwrap_or_else(|| {
            panic!("missing union generated-write guard for {call}; diagnostics: {diagnostics:?}")
        });
        assert!(
            !diagnostic.span.is_synthetic(),
            "guard must retain a source location: {diagnostic:?}"
        );
    }
}

#[test]
fn generated_bulk_writes_in_view_and_test_roots_are_located() {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;
    use roundhouse::project::BuildTarget;

    let roots = [
        (
            "app/views/people/index.html.erb",
            "<% Person.insert_all([{ first_name: \"Bulk\", last_name: \"View\", display_name: \"forged\" }]) %>\n",
        ),
        (
            "test/models/person_generated_column_test.rb",
            "class PersonGeneratedColumnTest < ActiveSupport::TestCase\n  test \"bulk write\" do\n    Person.insert_all([{ first_name: \"Bulk\", last_name: \"Test\", display_name: \"forged\" }])\n  end\nend\n",
        ),
    ];

    for (path, source_text) in roots {
        let (_emitted, analyzed_app, diagnostics) = app()
            .write(path, source_text)
            .emit_with_app(BuildTarget::Ruby);
        let diagnostic = diagnostics.iter().find(|diagnostic| {
            if diagnostic.severity != Severity::Error
                || !diagnostic.message.to_lowercase().contains("generated")
            {
                return false;
            }
            roundhouse::ide::source(&analyzed_app, diagnostic.span.file).is_some_and(|source| {
                Path::new(&source.path).ends_with(path)
                    && source
                        .text
                        .lines()
                        .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                        .is_some_and(|line| line.contains("Person.insert_all"))
            })
        });
        let diagnostic = diagnostic.unwrap_or_else(|| {
            panic!("missing located generated-write guard for {path}; diagnostics: {diagnostics:?}")
        });
        assert!(
            !diagnostic.span.is_synthetic(),
            "guard must retain a source location: {diagnostic:?}"
        );
    }
}

fn app_with_association_touch(person_body: &str, association: &str) -> emit_and_run::Overlay {
    let mut schema = include_str!("support/generated_columns_schema.rb").to_string();
    schema = schema.replacen(
        "    t.string \"last_name\"\n",
        "    t.string \"last_name\"\n    t.datetime \"last_seen_at\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n",
        1,
    );
    let schema_end = schema.rfind("end\n").expect("schema's outer end");
    schema.insert_str(
        schema_end,
        "  create_table \"comments\", force: :cascade do |t|\n    t.integer \"person_id\"\n    t.integer \"notifiable_id\"\n    t.string \"notifiable_type\"\n    t.datetime \"created_at\", null: false\n    t.datetime \"updated_at\", null: false\n  end\n\n",
    );
    app()
        .write("db/schema.rb", &schema)
        .write(
            "app/models/person.rb",
            &format!("class Person < ApplicationRecord\n{person_body}end\n"),
        )
        .write(
            "app/models/comment.rb",
            &format!("class Comment < ApplicationRecord\n  {association}\nend\n"),
        )
}

fn assert_association_touch_guard(
    app: &roundhouse::App,
    diagnostics: &[roundhouse::diagnostic::Diagnostic],
    expected_source_line: &str,
) {
    use std::path::Path;

    use roundhouse::diagnostic::Severity;

    let diagnostic = diagnostics.iter().find(|diagnostic| {
        if diagnostic.severity != Severity::Error
            || !diagnostic
                .message
                .to_lowercase()
                .contains("generated column")
        {
            return false;
        }
        roundhouse::ide::source(app, diagnostic.span.file).is_some_and(|source| {
            Path::new(&source.path).ends_with("app/models/comment.rb")
                && source
                    .text
                    .lines()
                    .nth(source.line_col(diagnostic.span.start).0.saturating_sub(1) as usize)
                    .is_some_and(|line| line.contains(expected_source_line))
        })
    });
    let diagnostic = diagnostic.unwrap_or_else(|| {
        panic!(
            "missing source-located generated association-touch guard for {expected_source_line}; diagnostics: {diagnostics:?}"
        )
    });
    assert!(
        !diagnostic.span.is_synthetic(),
        "association guard must retain the source declaration span: {diagnostic:?}"
    );
}

#[test]
fn association_touch_guards_generated_targets_and_preserves_ordinary_columns() {
    use roundhouse::project::BuildTarget;

    let (_emitted, analyzed_app, diagnostics) = app_with_association_touch(
        "",
        "belongs_to :author, class_name: \"Person\", foreign_key: :person_id, touch: :display_name",
    )
    .emit_with_app(BuildTarget::Ruby);
    assert_association_touch_guard(&analyzed_app, &diagnostics, "touch: :display_name");

    let (_emitted, _analyzed_app, diagnostics) = app_with_association_touch(
        "",
        "belongs_to :author, class_name: \"Person\", foreign_key: :person_id, touch: :last_seen_at",
    )
    .emit_with_app(BuildTarget::Ruby);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != roundhouse::diagnostic::Severity::Error),
        "ordinary belongs_to touch should remain supported: {diagnostics:?}"
    );
}

#[test]
fn polymorphic_association_touch_guards_resolved_and_unresolved_generated_targets() {
    use roundhouse::dialect::Association;
    use roundhouse::project::BuildTarget;

    let association = "belongs_to :notifiable, polymorphic: true, touch: :display_name";
    let (_emitted, analyzed_app, diagnostics) =
        app_with_association_touch("  has_many :comments, as: :notifiable\n", association)
            .emit_with_app(BuildTarget::Ruby);
    let comment = analyzed_app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Comment")
        .expect("Comment model");
    let Association::BelongsTo {
        polymorphic_targets,
        ..
    } = comment
        .associations()
        .find(|association| association.name().as_str() == "notifiable")
        .expect("notifiable association")
    else {
        panic!("expected polymorphic belongs_to");
    };
    assert_eq!(
        polymorphic_targets
            .iter()
            .map(|target| target.0.as_str())
            .collect::<Vec<_>>(),
        vec!["Person"],
        "resolved inverse target must exercise the resolved-target guard branch"
    );
    assert_association_touch_guard(&analyzed_app, &diagnostics, "touch: :display_name");

    let (_emitted, analyzed_app, diagnostics) =
        app_with_association_touch("", association).emit_with_app(BuildTarget::Ruby);
    let comment = analyzed_app
        .models
        .iter()
        .find(|model| model.name.0.as_str() == "Comment")
        .expect("Comment model");
    let Association::BelongsTo {
        polymorphic_targets,
        ..
    } = comment
        .associations()
        .find(|association| association.name().as_str() == "notifiable")
        .expect("notifiable association")
    else {
        panic!("expected polymorphic belongs_to");
    };
    assert!(
        polymorphic_targets.is_empty(),
        "no inverse or type literal should exercise the unresolved-target guard branch"
    );
    assert_association_touch_guard(&analyzed_app, &diagnostics, "touch: :display_name");

    let (_emitted, _analyzed_app, diagnostics) = app_with_association_touch(
        "  has_many :comments, as: :notifiable\n",
        "belongs_to :notifiable, polymorphic: true, touch: :last_seen_at",
    )
    .emit_with_app(BuildTarget::Ruby);
    assert!(
        diagnostics
            .iter()
            .all(|diagnostic| diagnostic.severity != roundhouse::diagnostic::Severity::Error),
        "ordinary polymorphic touch should remain supported: {diagnostics:?}"
    );
}
