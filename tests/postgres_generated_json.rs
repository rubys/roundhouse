//! Bounded PostgreSQL-only JSON extraction for generated-column DDL.
//!
//! These tests exercise schema validation and DDL rendering only. The explicit
//! PostgreSQL expression mode is not a PostgreSQL application runtime and does
//! not relax the ordinary SQLite/project gates.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use roundhouse::Symbol;
use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::structure_sql::{
    ingest_structure_sql, ingest_structure_sql_with_generated_expression_dialect,
};
use roundhouse::ingest::{
    ingest_app_from_tree, ingest_migration, ingest_schema,
    ingest_migration_with_generated_expression_dialect,
    ingest_schema_with_generated_expression_dialect, survey,
};
use roundhouse::project::{BuildTarget, target_files};
use roundhouse::schema::generated::GeneratedExpressionDialect;
use roundhouse::schema::{Column, ColumnType, Schema, Table};

const DDL_BEGIN: &str = "ROUNDHOUSE_POSTGRES_JSON_EXTRACTION_DDL_BEGIN";
const DDL_END: &str = "ROUNDHOUSE_POSTGRES_JSON_EXTRACTION_DDL_END";

/// Quotes the fixture expression for a Rails schema literal by escaping backslashes and double quotes.
fn ruby_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Builds a Rails schema fixture with stable JSON, text, optional source columns, and supplied generated expressions.
fn schema_source(extra_columns: &[&str], generated: &[(&str, &str)]) -> String {
    let mut lines = vec![
        "    t.json \"payload_json\"".to_string(),
        "    t.jsonb \"payload_jsonb\"".to_string(),
        "    t.string \"prefix\"".to_string(),
        "    t.string \"suffix\"".to_string(),
        "    t.json \"PayloadCase\"".to_string(),
        "    t.string \"dynamic_key\"".to_string(),
    ];
    lines.extend(extra_columns.iter().map(|line| format!("    {line}")));
    lines.extend(generated.iter().map(|(name, expression)| {
        format!(
            "    t.virtual \"{name}\", type: :string, as: {}, stored: true",
            ruby_string(expression)
        )
    }));
    format!(
        "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"documents\", force: :cascade do |t|\n{}\n  end\nend\n",
        lines.join("\n")
    )
}

/// Runs schema ingestion in the explicit PostgreSQL expression mode and returns user-facing error text.
fn ingest_postgres(source: &str) -> Result<Schema, String> {
    ingest_schema_with_generated_expression_dialect(
        source.as_bytes(),
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    )
    .map_err(|error| error.to_string())
}

/// Looks up a named table in test schema IR and fails with a useful fixture diagnostic.
fn table<'a>(schema: &'a Schema, name: &str) -> &'a Table {
    schema
        .tables
        .get(&Symbol::from(name))
        .unwrap_or_else(|| panic!("missing table {name}"))
}

/// Looks up a named table column and reports the table when the fixture is malformed.
fn column<'a>(table: &'a Table, name: &str) -> &'a Column {
    table
        .columns
        .iter()
        .find(|column| column.name.as_str() == name)
        .unwrap_or_else(|| panic!("missing column {name} on {}", table.name.as_str()))
}

/// Returns retained source SQL only for a generated column, making expression-preservation assertions concise.
fn generated_expression<'a>(table: &'a Table, name: &str) -> &'a str {
    column(table, name)
        .generated
        .as_ref()
        .unwrap_or_else(|| panic!("{}.{name} is not generated", table.name.as_str()))
        .expression
        .as_str()
}

/// Ingests a PostgreSQL-mode schema and renders its statements with the PostgreSQL DDL dialect.
fn postgres_ddl(source: &str) -> (Schema, Vec<String>) {
    let schema = ingest_postgres(source).expect("explicit PostgreSQL schema should ingest");
    let statements = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("explicit PostgreSQL generated-expression DDL should render");
    (schema, statements)
}

/// Returns the explicit PostgreSQL-mode validation error for a deliberately unsupported expression.
fn pg_error(source: &str) -> String {
    ingest_postgres(source).expect_err("unsupported PostgreSQL JSON expression").to_string()
}

/// Requires the rendered statement to contain the exact generated-column type and unrewritten expression.
fn assert_ddl_column(statement: &str, name: &str, expression: &str) {
    let line = format!(
        "\"{name}\" character varying GENERATED ALWAYS AS ({expression}) STORED"
    );
    assert!(statement.contains(&line), "missing exact DDL line {line:?}: {statement}");
}

/// Checks JSON kinds and source SQL through ingestion, serde round-trip, and exact PostgreSQL DDL rendering.
#[test]
fn postgres_json_extraction_retains_schema_serde_and_exact_ddl() {
    let expressions = [
        ("apostrophe_key", "payload_json ->> 'customer''s name'::text"),
        ("unicode_key", "payload_jsonb ->> '雪'::text"),
        ("issue_first", "((payload_json -> 'issue'::text) ->> 0)"),
        ("issue_message", "payload_json -> 'issue'::text ->> 1"),
        ("issue_last", "payload_jsonb -> 'issues'::text ->> -1"),
        (
            "item_label",
            "payload_json -> 'items'::text -> 0 ->> 'label'::text",
        ),
        ("quoted_source", "\"PayloadCase\" ->> 'Key'::text"),
        ("zero_key", "payload_json ->> '0'::text"),
        ("zero_index", "payload_json ->> 0"),
        ("suffix_concat", "payload_json ->> 'model'::text || suffix"),
        (
            "prefix_concat",
            "prefix || (payload_json ->> 'model'::text)",
        ),
        ("coalesced", "coalesce(payload_json ->> 'model'::text, '')"),
        (
            "casted_text",
            "CAST((payload_json ->> 'model'::text) AS text)",
        ),
    ];
    let source = schema_source(&[], &expressions);
    let (schema, statements) = postgres_ddl(&source);
    let documents = table(&schema, "documents");
    assert_eq!(column(documents, "payload_json").col_type, ColumnType::Json);
    assert_eq!(column(documents, "payload_jsonb").col_type, ColumnType::Jsonb);
    for (name, expression) in expressions {
        assert_eq!(generated_expression(documents, name), expression, "{name}");
        assert_eq!(
            column(documents, name).col_type,
            ColumnType::String { limit: None },
            "all extraction outputs remain text/string"
        );
    }

    let serialized = serde_json::to_value(&schema).expect("schema serializes");
    let round_trip: Schema = serde_json::from_value(serialized).expect("schema deserializes");
    assert_eq!(round_trip, schema, "serde retains exact source SQL and JSON types");
    let round_trip_documents = table(&round_trip, "documents");
    assert_eq!(column(round_trip_documents, "payload_json").col_type, ColumnType::Json);
    assert_eq!(column(round_trip_documents, "payload_jsonb").col_type, ColumnType::Jsonb);
    for (name, expression) in expressions {
        assert_eq!(generated_expression(round_trip_documents, name), expression);
    }

    assert_eq!(statements.len(), 1);
    let ddl = &statements[0];
    assert!(ddl.contains("\"payload_json\" json,"), "{ddl}");
    assert!(ddl.contains("\"payload_jsonb\" jsonb,"), "{ddl}");
    for (name, expression) in expressions {
        assert_ddl_column(ddl, name, expression);
    }
}

/// Emits marked DDL and oracle SQL for a live PostgreSQL check of key extraction, signed indexes, and text outputs.
#[test]
fn postgres_json_extraction_ddl_oracle_fixture_emits_marked_sql() {
    let expressions = [
        ("model_json", "payload_json ->> 'model'::text"),
        ("model_jsonb", "payload_jsonb ->> 'model'::text"),
        (
            "issue_first",
            "(payload_json -> 'issue'::text) ->> 0",
        ),
        (
            "issue_last_jsonb",
            "payload_jsonb -> 'issue'::text ->> -1",
        ),
        (
            "names_min_index_json",
            "payload_json -> 'names'::text ->> -2147483648",
        ),
        (
            "names_min_index_jsonb",
            "payload_jsonb -> 'names'::text ->> -2147483648",
        ),
        ("item_label", "payload_json -> 'items'::text -> 0 ->> 'label'::text"),
        ("names_first", "payload_json -> 'names'::text ->> 0"),
        ("names_last", "payload_json -> 'names'::text ->> -1"),
        (
            "array_text_key",
            "payload_json -> 'names'::text ->> '0'::text",
        ),
        ("zero_key", "payload_json ->> '0'::text"),
        ("zero_index", "payload_json ->> 0"),
        (
            "customer_key",
            "payload_json ->> 'customer''s name'::text",
        ),
        ("unicode_key", "payload_json ->> '雪'::text"),
        ("escaped_text", "payload_json ->> 'escaped'::text"),
        ("boolean_text", "payload_json ->> 'truth'::text"),
        ("number_text", "payload_json ->> 'count'::text"),
        (
            "missing_text",
            "payload_json ->> 'missing'::text",
        ),
        (
            "wrong_selector_text",
            "payload_json ->> 0",
        ),
        (
            "text_selector_array",
            "payload_json -> 'names'::text ->> 'missing'::text",
        ),
        (
            "empty_negative_index",
            "payload_json -> 'empty'::text ->> -1",
        ),
        (
            "scalar_descent",
            "payload_json -> 'scalar'::text ->> 'child'::text",
        ),
        (
            "json_nested_text",
            "payload_json ->> 'nested'::text",
        ),
        (
            "jsonb_nested_text",
            "payload_jsonb ->> 'nested'::text",
        ),
    ];
    let source = schema_source(&["t.string \"case_name\", null: false"], &expressions);
    let source = source.replace(
        "  end\nend\n",
        concat!(
            "  end\n",
            "  create_table \"required_documents\", force: :cascade do |t|\n",
            "    t.json \"payload_json\"\n",
            "    t.virtual \"required_model\", type: :string, as: \"payload_json ->> 'model'::text\", stored: true, null: false\n",
            "  end\nend\n"
        ),
    );
    let schema = ingest_postgres(&source).expect("oracle fixture schema should ingest");
    let statements = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("oracle fixture DDL should render");
    assert_eq!(statements.len(), 2);
    let ddl = statements
        .iter()
        .find(|statement| statement.starts_with("CREATE TABLE IF NOT EXISTS \"documents\""))
        .expect("documents emitted table DDL");
    let required_ddl = statements
        .iter()
        .find(|statement| statement.starts_with("CREATE TABLE IF NOT EXISTS \"required_documents\""))
        .expect("required_documents emitted table DDL");
    assert!(ddl.contains("\"payload_json\" json,"), "{ddl}");
    assert!(ddl.contains("\"payload_jsonb\" jsonb,"), "{ddl}");
    for (name, expression) in expressions {
        assert_ddl_column(ddl, name, expression);
    }
    assert!(required_ddl.contains("\"payload_json\" json,"), "{required_ddl}");
    assert!(required_ddl.contains("\"required_model\" character varying GENERATED ALWAYS AS (payload_json ->> 'model'::text) STORED NOT NULL"), "{required_ddl}");

    // With --nocapture this emits both blocks needed for a live database check.
    // On a disposable PostgreSQL database, run BEGIN, the marked DDL, the
    // marked oracle SQL, then ROLLBACK, using psql -X -v ON_ERROR_STOP=1.
    println!("{DDL_BEGIN}");
    for statement in &statements {
        println!("{statement};");
    }
    println!("{DDL_END}");
    println!("ROUNDHOUSE_POSTGRES_JSON_EXTRACTION_ORACLE_SQL_BEGIN");
    print!("{}", include_str!("support/postgres_generated_json_oracle.sql"));
    println!("ROUNDHOUSE_POSTGRES_JSON_EXTRACTION_ORACLE_SQL_END");
}

/// Checks that structure.sql retains JSON versus JSONB and extraction source text while Portable ingestion still rejects the operators.
#[test]
fn structure_sql_preserves_json_types_and_extraction_source_sql() {
    let structure = r#"CREATE TABLE public.documents (
  payload_json json,
  payload_jsonb jsonb,
  extracted text GENERATED ALWAYS AS ((payload_jsonb -> 'issue'::text) ->> 'detail'::text) STORED
);"#;
    let schema = ingest_structure_sql_with_generated_expression_dialect(
        structure.as_bytes(),
        "db/structure.sql",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("PostgreSQL structure.sql extraction should ingest");
    let documents = table(&schema, "documents");
    assert_eq!(column(documents, "payload_json").col_type, ColumnType::Json);
    assert_eq!(column(documents, "payload_jsonb").col_type, ColumnType::Jsonb);
    assert_eq!(
        generated_expression(documents, "extracted"),
        "(payload_jsonb -> 'issue'::text) ->> 'detail'::text"
    );

    let portable = ingest_structure_sql(structure.as_bytes(), "db/structure.sql")
        .expect_err("ordinary structure.sql ingestion remains Portable")
        .to_string();
    assert!(portable.contains("PostgreSQL JSON operators are unsupported"), "{portable}");
}

/// Checks JSON type changes and generated expressions across migration folding, with the ordinary migration path still Portable.
#[test]
fn migration_folding_preserves_json_kinds_and_expression_source() {
    let mut schema = ingest_postgres(&schema_source(&[], &[])).expect("base schema");
    let migration = r#"class EvolveDocuments < ActiveRecord::Migration[8.1]
  def change
    add_column :documents, :added_json, :json
    add_column :documents, :added_jsonb, :jsonb
    add_column :documents, :nested_label, :string, as: "payload_json -> 'issue'::text ->> 0", stored: true
    change_column :documents, :payload_jsonb, :json
  end
end
"#;
    ingest_migration_with_generated_expression_dialect(
        migration.as_bytes(),
        "db/migrate/evolve_documents.rb",
        &mut schema,
        GeneratedExpressionDialect::Postgres,
    )
    .expect("PostgreSQL JSON migration fold should succeed");

    let documents = table(&schema, "documents");
    assert_eq!(column(documents, "payload_json").col_type, ColumnType::Json);
    assert_eq!(column(documents, "payload_jsonb").col_type, ColumnType::Json);
    assert_eq!(column(documents, "added_json").col_type, ColumnType::Json);
    assert_eq!(column(documents, "added_jsonb").col_type, ColumnType::Jsonb);
    assert_eq!(
        generated_expression(documents, "nested_label"),
        "payload_json -> 'issue'::text ->> 0"
    );

    let portable_source = schema_source(&[], &[]);
    let mut portable_schema = ingest_schema(portable_source.as_bytes(), "db/schema.rb")
        .expect("portable JSON-only schema should ingest");
    let portable_error = ingest_migration(
        r#"class AddPortableGate < ActiveRecord::Migration[8.1]
  def change
    add_column :documents, :not_portable, :string, as: "payload_json ->> 'x'::text", stored: true
  end
end
"#
        .as_bytes(),
        "db/migrate/add_portable_gate.rb",
        &mut portable_schema,
    )
    .expect_err("ordinary migration ingest remains Portable")
    .to_string();
    assert!(portable_error.contains("not_portable"), "{portable_error}");
}

/// Pairs accepted literal keys and int4 endpoints with rejected dynamic selectors, malformed numbers, and out-of-range indexes.
#[test]
fn postgres_key_and_signed_index_selectors_obey_the_supported_int4_boundary() {
    for expression in [
        "payload_json ->> 0",
        "payload_json ->> -1",
        "payload_json ->> 2147483647",
        "payload_json ->> -2147483647",
        "payload_json ->> -2147483648",
        "payload_jsonb ->> -2147483648",
        "payload_json ->> '0'::text",
        "payload_json ->> ('model'::text)",
        "(payload_json ->> 'model'::text)::text",
    ] {
        let source = schema_source(&[], &[("selected", expression)]);
        let schema = ingest_postgres(&source).unwrap_or_else(|error| {
            panic!("valid literal selector {expression:?} should ingest: {error}")
        });
        assert_eq!(generated_expression(table(&schema, "documents"), "selected"), expression);
    }

    for expression in [
        "payload_json ->> 2147483648",
        "payload_json ->> -2147483649",
        "payload_json ->> 1.0",
        "payload_json ->> 1e0",
        "payload_json ->> '0'::integer",
        "payload_json ->> 0::text",
        "payload_json ->> dynamic_key",
        "payload_json ->> NULL",
        "payload_json ->> TRUE",
        "payload_json ->> lower(dynamic_key)",
        "payload_json ->> ARRAY[0]",
        "payload_json ->> $1",
    ] {
        let error = pg_error(&schema_source(&[], &[("selected", expression)]));
        assert!(error.contains("selected"), "{expression}: {error}");
    }
}

/// Covers wrong source types, generated or unknown references, unsupported SQL forms, and PostgreSQL operator-tier grouping.
#[test]
fn postgres_json_extraction_checks_types_provenance_and_operator_precedence() {
    for (extra, expression) in [
        ("t.string \"plain_text\"", "plain_text ->> 'key'::text"),
        ("t.integer \"number\"", "number ->> 'key'::text"),
        ("t.boolean \"enabled\"", "enabled ->> 'key'::text"),
        ("t.inet \"remote_ip\"", "remote_ip ->> 'key'::text"),
        ("t.enum \"status\"", "status ->> 'key'::text"),
        (
            "t.string \"plain_text\"",
            "plain_text -> 'key'::text ->> 'nested'::text",
        ),
        (
            "t.string \"plain_text\"",
            "payload_json::text ->> 'key'::text",
        ),
        (
            "t.string \"plain_text\"",
            "payload_jsonb || '{}'",
        ),
        (
            "t.string \"plain_text\"",
            "payload_json -> 'key'::text",
        ),
    ] {
        let source = schema_source(&[extra], &[("invalid_result", expression)]);
        let error = pg_error(&source);
        assert!(error.contains("invalid_result"), "{expression}: {error}");
    }

    for expression in [
        "missing_column ->> 'key'::text",
        "payload_json #> '{key}'",
        "payload_json #>> '{key}'",
        "payload_json ? 'key'",
        "payload_json @> '{}'::json",
        "payload_json ->> 'key'::text; DROP TABLE documents",
        "payload_json ->> 'key'::text /* trailing comment */",
        "payload_json ->> E'key'",
        "payload_json ->> 'line\\break'::text",
    ] {
        let error = pg_error(&schema_source(&[], &[("invalid_result", expression)]));
        assert!(error.contains("invalid_result"), "{expression}: {error}");
    }

    let wrong_case = pg_error(&schema_source(
        &[],
        &[("invalid_result", "PayloadCase ->> 'key'::text")],
    ));
    assert!(wrong_case.contains("Payloadcase") || wrong_case.contains("unknown column"), "{wrong_case}");

    let generated_reference = schema_source(
        &[],
        &[
            ("seed", "payload_json ->> 'seed'::text"),
            (
                "invalid_result",
                "coalesce(payload_json ->> 'key'::text, seed)",
            ),
        ],
    );
    let generated_error = pg_error(&generated_reference);
    assert!(generated_error.contains("generated column `seed`"), "{generated_error}");

    let integer_result = schema_source(
        &[],
        &[("integer_result", "payload_json ->> 'count'::text")],
    )
    .replace(
        "t.virtual \"integer_result\", type: :string",
        "t.virtual \"integer_result\", type: :integer",
    );
    let integer_error = pg_error(&integer_result);
    assert!(integer_error.contains("integer_result"), "{integer_error}");

    let invalid_left_association = pg_error(&schema_source(
        &[],
        &[("left_associated", "prefix || payload_json ->> 'model'::text")],
    ));
    assert!(
        invalid_left_association.contains("left_associated")
            && invalid_left_association.contains("||"),
        "operator-tier grouping should validate `prefix || payload_json` before extraction: {invalid_left_association}"
    );
}

/// Keeps JSON operators out of normal schema, application, project, and SQLite paths after opt-in DDL validation.
#[test]
fn portable_app_project_and_sqlite_paths_reject_postgres_json_extraction() {
    let source = schema_source(&[], &[("display", "payload_json ->> 'model'::text")]);
    let portable_schema_error = ingest_schema(source.as_bytes(), "db/schema.rb")
        .expect_err("normal schema ingestion remains Portable")
        .to_string();
    assert!(portable_schema_error.contains("display"), "{portable_schema_error}");

    let tree: HashMap<PathBuf, Vec<u8>> = [
        (
            "app/models/application_record.rb",
            "class ApplicationRecord < ActiveRecord::Base\n  self.abstract_class = true\nend\n",
        ),
        (
            "app/controllers/application_controller.rb",
            "class ApplicationController < ActionController::Base\nend\n",
        ),
        ("db/schema.rb", source.as_str()),
    ]
    .iter()
    .map(|(path, content)| (PathBuf::from(path), content.as_bytes().to_vec()))
    .collect();
    let app_error = ingest_app_from_tree(tree)
        .expect_err("normal application ingestion remains Portable")
        .to_string();
    assert!(app_error.contains("display"), "{app_error}");

    let (schema, _) = postgres_ddl(&source);
    let sqlite_error = render_schema_statements_for(&schema, Dialect::Sqlite)
        .expect_err("SQLite DDL rejects PostgreSQL JSON operators")
        .to_string();
    assert!(sqlite_error.contains("PostgreSQL JSON operators are unsupported"), "{sqlite_error}");

    let mut app = roundhouse::App::new();
    app.schema = schema;
    let project_error = target_files(&app, Path::new("."), BuildTarget::Ruby)
        .expect_err("project emission is still gated by SQLite expression support");
    assert!(project_error.contains("PostgreSQL JSON operators are unsupported"), "{project_error}");
}

/// Checks that survey drops only the bad output and invalid source mutations leave the last valid schema unchanged.
#[test]
fn schema_survey_and_migration_candidate_rejection_preserve_valid_state() {
    let source = schema_source(
        &[],
        &[
            ("valid_label", "payload_json ->> 'label'::text"),
            ("unsupported_label", "payload_json ->> dynamic_key"),
        ],
    );
    survey::activate();
    let surveyed = ingest_schema_with_generated_expression_dialect(
        source.as_bytes(),
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    );
    let schema_gaps = survey::drain();
    let surveyed = surveyed.expect("survey mode returns the table with supported state");
    let documents = table(&surveyed, "documents");
    assert!(documents.columns.iter().any(|column| column.name.as_str() == "valid_label"));
    assert!(documents.columns.iter().all(|column| column.name.as_str() != "unsupported_label"));
    assert_eq!(schema_gaps.len(), 1, "only the invalid output is ledgered: {schema_gaps:?}");
    assert!(schema_gaps[0].to_string().contains("unsupported_label"), "{schema_gaps:?}");

    let base = ingest_postgres(&schema_source(
        &[],
        &[("display", "payload_json ->> 'model'::text")],
    ))
    .expect("valid PostgreSQL generated schema");
    let original = base.clone();
    let migration = r#"class ChangeJsonSource < ActiveRecord::Migration[8.1]
  def change
    change_column :documents, :payload_json, :text
  end
end
"#;
    let mut rejected_schema = original.clone();
    let error = ingest_migration_with_generated_expression_dialect(
        migration.as_bytes(),
        "db/migrate/change_json_source.rb",
        &mut rejected_schema,
        GeneratedExpressionDialect::Postgres,
    )
    .expect_err("changing an extraction operand to text invalidates the candidate")
    .to_string();
    assert!(error.contains("generated column mutation"), "{error}");
    assert_eq!(rejected_schema, original, "failed strict migration is atomic");

    let mut surveyed_schema = original.clone();
    survey::activate();
    let result = survey::unwrap_or_record(ingest_migration_with_generated_expression_dialect(
        migration.as_bytes(),
        "db/migrate/change_json_source.rb",
        &mut surveyed_schema,
        GeneratedExpressionDialect::Postgres,
    ));
    let migration_gaps = survey::drain();
    assert!(matches!(result, Ok(None)), "survey skips only the rejected operation: {result:?}");
    assert_eq!(surveyed_schema, original, "candidate rejection must not partially mutate schema");
    assert_eq!(migration_gaps.len(), 1, "one failed operation has one gap: {migration_gaps:?}");
    assert!(migration_gaps[0].to_string().contains("change_column"), "{migration_gaps:?}");
}
