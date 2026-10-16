//! Generated columns keep their expression and storage mode in the schema,
//! reach dialect DDL, and reject forms that cannot be preserved portably.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::Symbol;
use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::{ingest_app_from_tree, ingest_schema, survey};
use roundhouse::project::{BuildTarget, target_files};
use roundhouse::schema::generated::GeneratedExpressionDialect;
use roundhouse::schema::{GeneratedColumnStorage, Schema, Table};

/// Builds the portable generated-column fixture used by the baseline behavior tests.
fn schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || coalesce(last_name, '')", stored: true
    t.virtual "normalized_first_name", type: :string, as: "coalesce(first_name, '')", stored: true, null: false
    t.virtual "fallback_name", type: :text, as: "coalesce(first_name, '')", stored: false
  end
end
"#,
        "db/schema.rb",
    )
    .expect("generated-column schema should ingest")
}

/// Finds a table by name and reports the missing name in test failures.
fn table<'a>(schema: &'a Schema, name: &str) -> &'a Table {
    schema
        .tables
        .get(&Symbol::from(name))
        .unwrap_or_else(|| panic!("missing table {name}"))
}

/// Finds a column by name and lists the available columns when an assertion fixture is incomplete.
fn column<'a>(table: &'a Table, name: &str) -> &'a roundhouse::schema::Column {
    table
        .columns
        .iter()
        .find(|column| column.name.as_str() == name)
        .unwrap_or_else(|| {
            panic!(
                "missing column {name} on {}; have {:?}",
                table.name.as_str(),
                table
                    .columns
                    .iter()
                    .map(|column| column.name.as_str())
                    .collect::<Vec<_>>()
            )
        })
}

fn generated_model_app() -> roundhouse::App {
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("db/schema.rb", include_str!("support/generated_columns_schema.rb")),
        ("app/models/person.rb", include_str!("support/generated_columns_person.rb")),
        (
            "app/models/virtual_person.rb",
            include_str!("support/generated_columns_virtual_person.rb"),
        ),
        (
            "app/models/constant_person.rb",
            include_str!("support/generated_columns_constant_person.rb"),
        ),
    ]
    .iter()
    .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
    .collect();
    let mut app = ingest_app_from_tree(tree).expect("ingest generated-column app");
    roundhouse::session::analyze_and_lower(&mut app);
    app
}

#[test]
fn generated_model_persistence_requires_a_runtime_with_insert_returning() {
    let app = generated_model_app();

    for target in [BuildTarget::Ruby, BuildTarget::Jruby, BuildTarget::Spinel] {
        target_files(&app, Path::new("."), target).unwrap_or_else(|error| {
            panic!("{target:?} runtime supports Db.exec_returning: {error}")
        });
    }

    let unsupported = [
        BuildTarget::Crystal,
        BuildTarget::Elixir,
        BuildTarget::Go,
        BuildTarget::Kotlin,
        BuildTarget::Python,
        BuildTarget::Rust,
        BuildTarget::Swift,
        BuildTarget::CSharp,
        BuildTarget::Typescript,
        BuildTarget::TypescriptWorker,
    ];
    for target in unsupported {
        let error = target_files(&app, Path::new("."), target).expect_err(
            "an SDK target without the persistence runtime must refuse generated models",
        );
        assert!(
            error.contains("generated-column model persistence"),
            "{target:?}: {error}"
        );
        assert!(error.contains(target.as_str()), "{target:?}: {error}");
        assert!(error.contains("Db.exec_returning"), "{target:?}: {error}");
        assert!(error.contains("display_name"), "{target:?}: {error}");
    }

    let roda_error = target_files(&app, Path::new("."), BuildTarget::Roda)
        .expect_err("Roda already rejects generated columns");
    assert!(
        roda_error.contains("Roda target does not support generated column"),
        "{roda_error}"
    );
}

#[test]
fn schema_rb_preserves_generated_expression_storage_and_nullability() {
    let schema = schema();
    let people = table(&schema, "people");

    let display_name = column(people, "display_name");
    let stored = display_name
        .generated
        .as_ref()
        .expect("stored generated metadata");
    assert_eq!(
        stored.expression,
        "first_name || ' ' || coalesce(last_name, '')"
    );
    assert_eq!(stored.storage, GeneratedColumnStorage::Stored);
    assert!(display_name.nullable);

    let fallback_name = column(people, "fallback_name");
    let virtual_column = fallback_name
        .generated
        .as_ref()
        .expect("virtual generated metadata");
    assert_eq!(virtual_column.expression, "coalesce(first_name, '')");
    assert_eq!(virtual_column.storage, GeneratedColumnStorage::Virtual);
    assert!(fallback_name.nullable);

    let normalized = column(people, "normalized_first_name");
    assert!(
        !normalized.nullable,
        "generated nullability remains a schema fact"
    );
    assert_eq!(
        normalized.generated.as_ref().unwrap().expression,
        "coalesce(first_name, '')"
    );
}

#[test]
fn sqlite_and_postgres_render_the_stored_expression_without_rewriting_it() {
    let schema = schema();
    let sqlite = render_schema_statements_for(&schema, Dialect::Sqlite).expect("SQLite DDL");
    assert!(
        sqlite[0].contains(
            "display_name TEXT GENERATED ALWAYS AS (first_name || ' ' || coalesce(last_name, '')) STORED"
        ),
        "stored expression should survive exactly: {sqlite:?}"
    );
    assert!(
        sqlite[0]
            .contains("fallback_name TEXT GENERATED ALWAYS AS (coalesce(first_name, '')) VIRTUAL"),
        "SQLite can render both generated storage modes: {sqlite:?}"
    );
    assert!(
        sqlite[0].contains(
            "normalized_first_name TEXT GENERATED ALWAYS AS (coalesce(first_name, '')) STORED NOT NULL"
        ),
        "generated nullability must be rendered after its storage mode: {sqlite:?}"
    );

    let postgres = render_schema_statements_for(&schema, Dialect::Postgres).expect_err(
        "Roundhouse PostgreSQL DDL renderer does not yet support virtual generated columns",
    );
    assert!(
        postgres.contains(
            "Roundhouse PostgreSQL DDL renderer does not yet support virtual generated columns"
        ),
        "the boundary should name Roundhouse's renderer, not imply PostgreSQL lacks the feature: {postgres}"
    );

    let stored_only = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "stored_people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || coalesce(last_name, '')", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("stored-only schema should ingest");
    let postgres =
        render_schema_statements_for(&stored_only, Dialect::Postgres).expect("Postgres stored DDL");
    assert!(
        postgres[0].contains(
            "\"display_name\" character varying GENERATED ALWAYS AS (first_name || ' ' || coalesce(last_name, '')) STORED"
        ),
        "Postgres DDL should retain the expression: {postgres:?}"
    );
}

/// Checks that explicit PostgreSQL DDL ingest preserves approved cast SQL while Portable, SQLite, and project paths remain gated.
#[test]
fn postgres_text_casts_are_an_explicit_ddl_only_ingest_path() {
    let source = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.text "last_name"
    t.virtual "display_name", type: :string, as: "(first_name || coalesce(last_name, ''))::text", stored: true
    t.virtual "standard_cast", type: :text, as: "CAST((first_name || last_name) AS character varying)", stored: true
    t.virtual "qualified_cast", type: :string, as: "first_name::character varying", stored: true
    t.virtual "short_cast", type: :string, as: "first_name::varchar", stored: true
    t.virtual "standard_varchar", type: :string, as: "CAST(last_name AS varchar)", stored: true
  end
end
"#;

    let schema = roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
        source,
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("the explicit PostgreSQL DDL path accepts whitelisted text casts");
    let people = table(&schema, "people");
    assert_eq!(
        column(people, "display_name")
            .generated
            .as_ref()
            .unwrap()
            .expression,
        "(first_name || coalesce(last_name, ''))::text"
    );
    assert_eq!(
        column(people, "standard_cast")
            .generated
            .as_ref()
            .unwrap()
            .expression,
        "CAST((first_name || last_name) AS character varying)"
    );

    let postgres = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("Postgres DDL accepts its bounded text-cast grammar");
    assert!(
        postgres[0].contains(
            "\"display_name\" character varying GENERATED ALWAYS AS ((first_name || coalesce(last_name, ''))::text) STORED"
        ),
        "the validated source cast should be emitted unchanged: {postgres:?}"
    );
    assert!(
        postgres[0].contains(
            "\"standard_cast\" text GENERATED ALWAYS AS (CAST((first_name || last_name) AS character varying)) STORED"
        ),
        "CAST syntax and nested concatenation should remain unchanged: {postgres:?}"
    );
    assert!(
        postgres[0].contains(
            "\"qualified_cast\" character varying GENERATED ALWAYS AS (first_name::character varying) STORED"
        ) && postgres[0].contains(
            "\"standard_varchar\" character varying GENERATED ALWAYS AS (CAST(last_name AS varchar)) STORED"
        ),
        "both PostgreSQL cast targets must be retained in either cast spelling: {postgres:?}"
    );

    let sqlite = render_schema_statements_for(&schema, Dialect::Sqlite)
        .expect_err("PostgreSQL-only text casts cannot reach SQLite DDL");
    assert!(sqlite.contains("PostgreSQL casts are unsupported"), "{sqlite}");

    let portable = ingest_schema(source, "db/schema.rb")
        .expect_err("normal app/schema ingest remains on the Portable grammar")
        .to_string();
    assert!(portable.contains("generated"), "{portable}");

    let mut app = roundhouse::App::new();
    app.schema = schema;
    let project_error = target_files(&app, Path::new("."), BuildTarget::Ruby)
        .expect_err("current project targets remain guarded by SQLite DDL support");
    assert!(
        project_error.contains("PostgreSQL casts are unsupported"),
        "{project_error}"
    );
}

/// Locks application ingestion to the Portable grammar even when a schema contains a PostgreSQL-only cast.
#[test]
fn normal_app_ingest_refuses_postgres_text_casts() {
    let schema = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "normalized_name", type: :string, as: "first_name::text", stored: true
  end
end
"#;
    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("db/schema.rb", std::str::from_utf8(schema).unwrap()),
    ]
    .iter()
    .map(|(path, source)| (PathBuf::from(path), source.as_bytes().to_vec()))
    .collect();

    let error = ingest_app_from_tree(tree)
        .expect_err("application ingest uses Portable expressions, not the DDL opt-in")
        .to_string();
    assert!(error.contains("normalized_name"), "{error}");
    assert!(error.contains("generated column dropped"), "{error}");
}

/// Emits exact cast DDL and marked oracle SQL for a disposable PostgreSQL execution, including NULL and Unicode cases.
#[test]
fn postgres_text_cast_ddl_oracle_fixture_emits_marked_sql() {
    let source = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "roundhouse_cast_oracle", force: :cascade do |t|
    t.string "Given Name"
    t.text "雪"
    t.virtual "postfix_text", type: :text, as: "\"Given Name\"::text", stored: true
    t.virtual "postfix_varchar", type: :string, as: "\"Given Name\"::varchar", stored: true
    t.virtual "postfix_character_varying", type: :string, as: "\"Given Name\"::character varying", stored: true
    t.virtual "cast_text", type: :text, as: "CAST(\"Given Name\" AS text)", stored: true
    t.virtual "cast_varchar", type: :string, as: "CAST(\"Given Name\" AS varchar)", stored: true
    t.virtual "cast_character_varying", type: :string, as: "CAST(\"Given Name\" AS character varying)", stored: true
    t.virtual "display_名", type: :text, as: "coalesce(\"Given Name\"::text, '') || ' — ' || \"雪\"::varchar", stored: true
  end
end
"#;
    let schema = roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
        source.as_bytes(),
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("quoted/unicode identifiers and text casts should ingest");
    let ddl = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("the Postgres renderer should retain the cast expression");
    assert_eq!(ddl.len(), 1);
    for (name, result_type, expression) in [
        ("postfix_text", "text", r#""Given Name"::text"#),
        (
            "postfix_varchar",
            "character varying",
            r#""Given Name"::varchar"#,
        ),
        (
            "postfix_character_varying",
            "character varying",
            r#""Given Name"::character varying"#,
        ),
        ("cast_text", "text", r#"CAST("Given Name" AS text)"#),
        (
            "cast_varchar",
            "character varying",
            r#"CAST("Given Name" AS varchar)"#,
        ),
        (
            "cast_character_varying",
            "character varying",
            r#"CAST("Given Name" AS character varying)"#,
        ),
        (
            "display_名",
            "text",
            r#"coalesce("Given Name"::text, '') || ' — ' || "雪"::varchar"#,
        ),
    ] {
        let expected = format!(
            "\"{name}\" {result_type} GENERATED ALWAYS AS ({expression}) STORED"
        );
        assert!(
            ddl[0].contains(&expected),
            "the oracle must execute emitted DDL verbatim; missing {expected:?} in {ddl:?}"
        );
    }

    // The parent oracle extracts the actual emitted DDL and this machine-
    // checked SQL from `--nocapture` output.
    println!("ROUNDHOUSE_PG_TEXT_CAST_DDL_BEGIN");
    println!("{};", ddl[0]);
    println!("ROUNDHOUSE_PG_TEXT_CAST_DDL_END");
    println!("ROUNDHOUSE_PG_TEXT_CAST_ORACLE_SQL_BEGIN");
    println!(
        r#"INSERT INTO "roundhouse_cast_oracle" ("Given Name", "雪") VALUES (NULL, '東京'), ('Ada', '東京'), ('', '雪');

DO $roundhouse_cast_oracle$
DECLARE
  actual RECORD;
BEGIN
  SELECT "postfix_text", "postfix_varchar", "postfix_character_varying",
         "cast_text", "cast_varchar", "cast_character_varying", "display_名" AS display_value
    INTO STRICT actual
    FROM "roundhouse_cast_oracle"
   WHERE "Given Name" IS NULL AND "雪" = '東京';
  IF actual.postfix_text IS DISTINCT FROM NULL::text
     OR actual.postfix_varchar IS DISTINCT FROM NULL::character varying
     OR actual.postfix_character_varying IS DISTINCT FROM NULL::character varying
     OR actual.cast_text IS DISTINCT FROM NULL::text
     OR actual.cast_varchar IS DISTINCT FROM NULL::character varying
     OR actual.cast_character_varying IS DISTINCT FROM NULL::character varying
     OR actual.display_value IS DISTINCT FROM ' — 東京'::text THEN
    RAISE EXCEPTION 'NULL input row mismatch: %', actual;
  END IF;

  SELECT "postfix_text", "postfix_varchar", "postfix_character_varying",
         "cast_text", "cast_varchar", "cast_character_varying", "display_名" AS display_value
    INTO STRICT actual
    FROM "roundhouse_cast_oracle"
   WHERE "Given Name" = 'Ada' AND "雪" = '東京';
  IF actual.postfix_text IS DISTINCT FROM 'Ada'::text
     OR actual.postfix_varchar IS DISTINCT FROM 'Ada'::character varying
     OR actual.postfix_character_varying IS DISTINCT FROM 'Ada'::character varying
     OR actual.cast_text IS DISTINCT FROM 'Ada'::text
     OR actual.cast_varchar IS DISTINCT FROM 'Ada'::character varying
     OR actual.cast_character_varying IS DISTINCT FROM 'Ada'::character varying
     OR actual.display_value IS DISTINCT FROM 'Ada — 東京'::text THEN
    RAISE EXCEPTION 'non-NULL input row mismatch: %', actual;
  END IF;

  SELECT "postfix_text", "postfix_varchar", "postfix_character_varying",
         "cast_text", "cast_varchar", "cast_character_varying", "display_名" AS display_value
    INTO STRICT actual
    FROM "roundhouse_cast_oracle"
   WHERE "Given Name" = '' AND "雪" = '雪';
  IF actual.postfix_text IS DISTINCT FROM ''::text
     OR actual.postfix_varchar IS DISTINCT FROM ''::character varying
     OR actual.postfix_character_varying IS DISTINCT FROM ''::character varying
     OR actual.cast_text IS DISTINCT FROM ''::text
     OR actual.cast_varchar IS DISTINCT FROM ''::character varying
     OR actual.cast_character_varying IS DISTINCT FROM ''::character varying
     OR actual.display_value IS DISTINCT FROM ' — 雪'::text THEN
    RAISE EXCEPTION 'empty-string input row mismatch: %', actual;
  END IF;
END;
$roundhouse_cast_oracle$;"#
    );
    println!("ROUNDHOUSE_PG_TEXT_CAST_ORACLE_SQL_END");
}

/// Exercises negative controls for numeric or limited targets, functions, comments, and trailing statements.
#[test]
fn postgres_cast_whitelist_rejects_other_targets_and_sql_forms() {
    for expression in [
        "first_name::integer",
        "first_name::varchar(12)",
        "CAST(first_name AS citext)",
        "lower(first_name)::text",
        "first_name /* comment */::text",
        "first_name::text; DROP TABLE people",
    ] {
        let source = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "casted", type: :string, as: "{expression}", stored: true
  end
end
"#
        );
        let error = roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
            source.as_bytes(),
            "db/schema.rb",
            GeneratedExpressionDialect::Postgres,
        )
        .expect_err("only whitelisted text casts should be admitted")
        .to_string();
        assert!(error.contains("casted"), "{expression}: {error}");
    }
}

/// Verifies survey mode removes an unsupported generated output and records exactly one gap.
#[test]
fn postgres_cast_survey_drops_the_bad_column_and_records_the_gap() {
    survey::activate();
    let schema = roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "casted", type: :string, as: "first_name::integer", stored: true
  end
end
"#,
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    );
    let gaps = survey::drain();

    let schema = schema.expect("survey mode keeps valid schema IR");
    assert!(
        table(&schema, "people")
            .columns
            .iter()
            .all(|column| column.name.as_str() != "casted"),
        "invalid generated output must be removed from survey IR"
    );
    assert_eq!(gaps.len(), 1, "the unsupported cast is recorded once: {gaps:?}");
    assert!(gaps[0].to_string().contains("casted"), "{gaps:?}");
}

#[test]
fn generated_columns_coexist_with_a_custom_string_primary_key() {
    let schema = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "custom_people", primary_key: "person_key", id: :string, force: :cascade do |t|
    t.string "first_name"
    t.virtual "label", type: :string, as: "first_name || '-custom'", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("custom string key and generated column should ingest");
    let custom_people = table(&schema, "custom_people");
    let key = column(custom_people, "person_key");
    assert!(key.primary_key);
    assert!(key.generated.is_none());
    let label = column(custom_people, "label");
    assert!(!label.primary_key);
    assert_eq!(
        label.generated.as_ref().unwrap().expression,
        "first_name || '-custom'"
    );

    let sqlite = render_schema_statements_for(&schema, Dialect::Sqlite).expect("SQLite DDL");
    assert!(
        sqlite[0].contains("person_key TEXT PRIMARY KEY NOT NULL"),
        "{sqlite:?}"
    );
    assert!(
        sqlite[0].contains("label TEXT GENERATED ALWAYS AS (first_name || '-custom') STORED"),
        "{sqlite:?}"
    );
    let postgres = render_schema_statements_for(&schema, Dialect::Postgres).expect("Postgres DDL");
    assert!(
        postgres[0].contains("\"person_key\" character varying PRIMARY KEY NOT NULL"),
        "{postgres:?}"
    );
    assert!(
        postgres[0].contains(
            "\"label\" character varying GENERATED ALWAYS AS (first_name || '-custom') STORED"
        ),
        "{postgres:?}"
    );
}

#[test]
fn unsupported_expressions_and_generated_column_dsl_fail_strict_ingest() {
    let unsupported = [
        (
            "first_name::text",
            "PostgreSQL casts are not rewritten into a different dialect",
        ),
        (
            "payload ->> 'name'",
            "PostgreSQL JSON operators are not rewritten into SQLite JSON1",
        ),
        (
            "lower(first_name)",
            "functions outside concat/coalesce are not in the portable subset",
        ),
        (
            "first_name || 'x' trailing",
            "trailing expression tokens must not be discarded",
        ),
        (
            "first_name || 'x\\0y'",
            "NUL cannot be embedded in a generated SQL expression",
        ),
    ];

    for (expression, reason) in unsupported {
        let source = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.text "payload"
    t.virtual "display_name", type: :string, as: "{expression}", stored: true
  end
end
"#
        );
        let error = ingest_schema(source.as_bytes(), "db/schema.rb")
            .expect_err("unsupported generated expressions must not become writable columns")
            .to_string();
        assert!(
            error.contains("display_name") && error.contains("generated"),
            "{reason}: {error}"
        );
    }

    let missing_options = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.virtual "display_name"
  end
end
"#,
        "db/schema.rb",
    )
    .expect_err("t.virtual without a result type and expression must fail explicitly")
    .to_string();
    assert!(
        missing_options.contains("generated column type is missing")
            || missing_options.contains("generated expression is missing"),
        "{missing_options}"
    );
}

#[test]
fn generated_expression_keyword_columns_must_be_quoted() {
    for keyword in ["ANY", "USER", "SOME"] {
        let column_name = keyword.to_ascii_lowercase();
        let unquoted = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "{column_name}"
    t.virtual "computed", type: :string, as: "{keyword}", stored: true
  end
end
"#
        );
        let error = ingest_schema(unquoted.as_bytes(), "db/schema.rb")
            .expect_err("bare SQL keywords must not be mistaken for portable column references")
            .to_string();
        assert!(
            error.contains("unquoted SQL keyword") && error.contains(keyword),
            "{keyword}: {error}"
        );

        let quoted = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "{column_name}"
    t.virtual "computed", type: :string, as: '"{column_name}"', stored: true
  end
end
"#
        );
        let schema = ingest_schema(quoted.as_bytes(), "db/schema.rb")
            .expect("a double-quoted SQL keyword is an identifier");
        let computed = column(table(&schema, "people"), "computed");
        assert_eq!(
            computed.generated.as_ref().unwrap().expression,
            format!("\"{column_name}\"")
        );

        let sqlite = render_schema_statements_for(&schema, Dialect::Sqlite).expect("SQLite DDL");
        assert!(
            sqlite[0].contains(&format!(
                "computed TEXT GENERATED ALWAYS AS (\"{column_name}\") STORED"
            )),
            "{keyword}: {sqlite:?}"
        );
        let postgres =
            render_schema_statements_for(&schema, Dialect::Postgres).expect("Postgres DDL");
        assert!(
            postgres[0].contains(&format!(
                "\"computed\" character varying GENERATED ALWAYS AS (\"{column_name}\") STORED"
            )),
            "{keyword}: {postgres:?}"
        );
    }
}

#[test]
fn unicode_literals_are_preserved_and_invalid_source_encoding_is_rejected() {
    let source = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "localized_name", type: :string, as: "coalesce(first_name, '未設定')", stored: true
  end
end
"#
    .as_bytes();
    let schema = ingest_schema(source, "db/schema.rb").expect("Unicode literal is valid SQL text");
    assert_eq!(
        column(table(&schema, "people"), "localized_name")
            .generated
            .as_ref()
            .unwrap()
            .expression,
        "coalesce(first_name, '未設定')"
    );

    let mut invalid_utf8 = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "localized_name", type: :string, as: "coalesce(first_name, '"#
        .to_vec();
    invalid_utf8.push(0xff);
    invalid_utf8.extend_from_slice(
        br#"')", stored: true
  end
end
"#,
    );
    let parsed = std::panic::catch_unwind(|| ingest_schema(&invalid_utf8, "db/schema.rb"));
    let result = parsed.expect("invalid UTF-8 in an expression must not panic ingest");
    assert!(
        result.is_err(),
        "invalid UTF-8 must be an explicit ingest error"
    );
}

/// Confirms that the explicit structure-DDL path preserves casts while the portable path rejects them.
#[test]
fn structure_sql_keeps_supported_generated_columns_and_rejects_casts() {
    let source = br#"CREATE TABLE public.people (
    id bigint NOT NULL,
    first_name text,
    last_name text,
    display_name text GENERATED ALWAYS AS (first_name || ' ' || coalesce(last_name, '')) STORED
);
ALTER TABLE ONLY public.people ADD CONSTRAINT people_pkey PRIMARY KEY (id);
"#;
    let schema =
        roundhouse::ingest::structure_sql::ingest_structure_sql(source, "db/structure.sql")
            .expect("portable generated expression in structure.sql");
    let people = table(&schema, "people");
    let generated = column(people, "display_name")
        .generated
        .as_ref()
        .expect("generated column");
    assert_eq!(
        generated.expression,
        "first_name || ' ' || coalesce(last_name, '')"
    );
    assert_eq!(generated.storage, GeneratedColumnStorage::Stored);
    let postgres = render_schema_statements_for(&schema, Dialect::Postgres).expect("Postgres DDL");
    assert!(
        postgres[0]
            .contains("GENERATED ALWAYS AS (first_name || ' ' || coalesce(last_name, '')) STORED")
    );

    let unicode = r#"CREATE TABLE public.people (
    id bigint NOT NULL,
    first_name text,
    display_name text GENERATED ALWAYS AS (coalesce(first_name, '未設定')) STORED
);
ALTER TABLE ONLY public.people ADD CONSTRAINT people_pkey PRIMARY KEY (id);
"#
    .as_bytes();
    let schema =
        roundhouse::ingest::structure_sql::ingest_structure_sql(unicode, "db/structure.sql")
            .expect("Unicode text literal should survive structure.sql ingest");
    assert_eq!(
        column(table(&schema, "people"), "display_name")
            .generated
            .as_ref()
            .unwrap()
            .expression,
        "coalesce(first_name, '未設定')"
    );

    let casted = br#"CREATE TABLE public.people (
    id bigint NOT NULL,
    first_name text,
    display_name text GENERATED ALWAYS AS (first_name::text) STORED
);
ALTER TABLE ONLY public.people ADD CONSTRAINT people_pkey PRIMARY KEY (id);
"#;
    let error = roundhouse::ingest::structure_sql::ingest_structure_sql(casted, "db/structure.sql")
        .expect_err("PG-specific casts remain an explicit unsupported boundary")
        .to_string();
    assert!(
        error.contains("generated column dropped: people.display_name"),
        "{error}"
    );

    let casted_schema = roundhouse::ingest::ingest_structure_sql_with_generated_expression_dialect(
        casted,
        "db/structure.sql",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("the explicit PostgreSQL DDL path accepts the bounded text cast");
    let postgres =
        render_schema_statements_for(&casted_schema, Dialect::Postgres).expect("Postgres DDL");
    assert!(
        postgres[0]
            .contains("\"display_name\" text GENERATED ALWAYS AS (first_name::text) STORED"),
        "structure.sql expression should survive unchanged: {postgres:?}"
    );
    assert!(
        render_schema_statements_for(&casted_schema, Dialect::Sqlite).is_err(),
        "the explicit PostgreSQL source mode must not bypass SQLite rendering validation"
    );

    let mut invalid_utf8 = br#"CREATE TABLE public.people (
    id bigint NOT NULL,
    first_name text,
    display_name text GENERATED ALWAYS AS (coalesce(first_name, '"#
        .to_vec();
    invalid_utf8.push(0xff);
    invalid_utf8.extend_from_slice(
        br#"')) STORED
);
ALTER TABLE ONLY public.people ADD CONSTRAINT people_pkey PRIMARY KEY (id);
"#,
    );
    let error =
        roundhouse::ingest::structure_sql::ingest_structure_sql(&invalid_utf8, "db/structure.sql")
            .expect_err("lossy invalid UTF-8 must not pass as a generated literal")
            .to_string();
    assert!(
        error.contains("generated column dropped: people.display_name"),
        "{error}"
    );
}

#[test]
fn survey_mode_ledgers_unsupported_generated_expressions() {
    survey::activate();
    let result = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "display_name", type: :string, as: "lower(first_name)", stored: true
  end
end
"#,
        "db/schema.rb",
    );
    let gaps = survey::drain();
    let schema = result.expect("survey mode should retain the rest of the schema");
    assert!(
        table(&schema, "people")
            .columns
            .iter()
            .all(|column| column.name.as_str() != "display_name"),
        "unsupported generated columns should be dropped in survey mode"
    );
    assert_eq!(
        gaps.len(),
        1,
        "unsupported generated column must be ledgered: {gaps:?}"
    );
    assert!(
        gaps[0]
            .to_string()
            .contains("generated column dropped: people.display_name")
    );
}
