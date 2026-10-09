//! Bounded PostgreSQL int4 results for generated-column DDL.
//!
//! The explicit PostgreSQL expression mode validates schema DDL only. These
//! tests preserve the existing Portable application and SQLite boundaries.

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

const DDL_BEGIN: &str = "ROUNDHOUSE_POSTGRES_GENERATED_INT4_DDL_BEGIN";
const DDL_END: &str = "ROUNDHOUSE_POSTGRES_GENERATED_INT4_DDL_END";
const INT4_EXPRESSION: &str = "(payload_jsonb ->> 'counter'::text)::integer";

/// Quotes the fixture expression for a Rails schema literal by escaping backslashes and double quotes.
fn ruby_string(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

/// Builds a counters schema with ordinary JSON and text sources plus caller-selected generated result types and expressions.
fn schema_source(
    extra_columns: &[&str],
    generated: &[(&str, &str, &str)],
) -> String {
    let mut lines = vec![
        "    t.json \"payload_json\"".to_string(),
        "    t.jsonb \"payload_jsonb\"".to_string(),
        "    t.text \"counter_text\"".to_string(),
        "    t.string \"label\"".to_string(),
    ];
    lines.extend(extra_columns.iter().map(|line| format!("    {line}")));
    lines.extend(generated.iter().map(|(name, type_options, expression)| {
        format!(
            "    t.virtual \"{name}\", type: {type_options}, as: {}, stored: true",
            ruby_string(expression)
        )
    }));
    format!(
        "ActiveRecord::Schema[8.1].define(version: 1) do\n  create_table \"counters\", force: :cascade do |t|\n{}\n  end\nend\n",
        lines.join("\n")
    )
}

/// Runs schema ingestion with the explicit PostgreSQL expression grammar.
fn ingest_postgres(source: &str) -> Result<Schema, String> {
    ingest_schema_with_generated_expression_dialect(
        source.as_bytes(),
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    )
    .map_err(|error| error.to_string())
}

/// Looks up a named fixture table in schema IR.
fn table<'a>(schema: &'a Schema, name: &str) -> &'a Table {
    schema
        .tables
        .get(&Symbol::from(name))
        .unwrap_or_else(|| panic!("missing table {name}"))
}

/// Looks up a named fixture column and includes the table name in failures.
fn column<'a>(table: &'a Table, name: &str) -> &'a Column {
    table
        .columns
        .iter()
        .find(|column| column.name.as_str() == name)
        .unwrap_or_else(|| panic!("missing column {name} on {}", table.name.as_str()))
}

/// Returns the retained SQL expression for a generated column.
fn generated_expression<'a>(table: &'a Table, name: &str) -> &'a str {
    column(table, name)
        .generated
        .as_ref()
        .unwrap_or_else(|| panic!("{}.{name} is not generated", table.name.as_str()))
        .expression
        .as_str()
}

/// Returns the validation error from an unsupported int4 expression shape.
fn pg_error(source: &str) -> String {
    ingest_postgres(source)
        .expect_err("unsupported generated int4 shape")
        .to_string()
}

/// Requires an exact PostgreSQL integer generated-column line with the original expression.
fn assert_int4_ddl(ddl: &str, name: &str, expression: &str) {
    let expected = format!(
        "\"{name}\" integer GENERATED ALWAYS AS ({expression}) STORED"
    );
    assert!(ddl.contains(&expected), "missing exact int4 DDL {expected:?}: {ddl}");
}

/// Checks supported casts through serde and exact DDL, while sparse markers distinguish ordinary integer widths that lost int4 proof.
#[test]
fn schema_rb_serde_and_postgres_ddl_preserve_int4_expressions_and_width_provenance() {
    let expressions = [
        (
            "json_integer",
            ":integer",
            "(payload_json ->> 'counter'::text)::integer",
        ),
        (
            "jsonb_int4",
            ":integer",
            "(payload_jsonb ->> 'counter'::text)::int4",
        ),
        (
            "cast_integer",
            ":integer",
            "CAST((payload_jsonb ->> 'counter'::text) AS integer)",
        ),
        (
            "cast_int4",
            ":integer",
            "CAST((payload_json ->> 'counter'::text) AS int4)",
        ),
        ("text_counter", ":integer", "counter_text::integer"),
        (
            "text_coalesce",
            ":integer",
            "coalesce(payload_jsonb ->> 'counter'::text, '0')::integer",
        ),
        (
            "rails_limit_three",
            ":integer, limit: 3",
            "(payload_jsonb ->> 'counter'::text)::integer",
        ),
        (
            "rails_limit_four",
            ":integer, limit: 4",
            "(payload_jsonb ->> 'counter'::text)::int4",
        ),
    ];
    let ordinary_widths = [
        "t.integer \"ordinary_limit_zero\", limit: 0",
        "t.integer \"ordinary_limit_one\", limit: 1",
        "t.integer \"ordinary_limit_two\", limit: 2",
        "t.integer \"ordinary_limit_three\", limit: 3",
        "t.integer \"ordinary_limit_four\", limit: 4",
        "t.integer \"ordinary_limit_five\", limit: 5",
        "t.integer \"ordinary_limit_eight\", limit: 8",
        "t.integer \"ordinary_limit_nine\", limit: 9",
        "t.integer \"ordinary_default\"",
    ];
    let source = schema_source(&ordinary_widths, &expressions);
    let schema = ingest_postgres(&source).expect("exact int4 schema should ingest");
    let counters = table(&schema, "counters");

    for (name, _, expression) in expressions {
        let generated = column(counters, name);
        assert_eq!(generated_expression(counters, name), expression, "{name}");
        assert_eq!(generated.col_type, ColumnType::Integer, "{name}");
        assert_eq!(generated.generated_int4_compatible, None, "exact int4: {name}");
    }
    for (name, expected_marker) in [
        ("ordinary_limit_zero", Some(false)),
        ("ordinary_limit_one", Some(false)),
        ("ordinary_limit_two", Some(false)),
        ("ordinary_limit_three", None),
        ("ordinary_limit_four", None),
        ("ordinary_limit_five", None),
        ("ordinary_limit_eight", None),
        ("ordinary_limit_nine", Some(false)),
        ("ordinary_default", None),
    ] {
        let source_column = column(counters, name);
        if matches!(name, "ordinary_limit_five" | "ordinary_limit_eight") {
            assert_eq!(source_column.col_type, ColumnType::BigInt, "{name}");
        } else {
            // Narrow and unusual Rails widths stay application-typed Integer;
            // the sparse marker records only the lost int4 source proof.
            assert_eq!(source_column.col_type, ColumnType::Integer, "{name}");
        }
        assert_eq!(source_column.generated_int4_compatible, expected_marker, "{name}");
    }

    let serialized = serde_json::to_value(&schema).expect("schema serializes");
    let counter_json = serialized["tables"]["counters"]["columns"]
        .as_array()
        .expect("serialized columns");
    let plain_integer_json = counter_json
        .iter()
        .find(|column| column["name"] == "ordinary_default")
        .expect("ordinary integer source in serde");
    assert!(
        plain_integer_json.get("generated_int4_compatible").is_none(),
        "None provenance should remain sparse: {plain_integer_json}"
    );
    let narrow_integer_json = counter_json
        .iter()
        .find(|column| column["name"] == "ordinary_limit_one")
        .expect("narrow integer source in serde");
    assert_eq!(narrow_integer_json["generated_int4_compatible"], false);
    let round_trip: Schema = serde_json::from_value(serialized).expect("schema deserializes");
    assert_eq!(round_trip, schema, "serde retains source SQL and int4 provenance");

    let statements = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("PostgreSQL DDL renders exact generated int4 results");
    assert_eq!(statements.len(), 1);
    let ddl = &statements[0];
    assert!(ddl.contains("\"payload_json\" json,"), "{ddl}");
    assert!(ddl.contains("\"payload_jsonb\" jsonb,"), "{ddl}");
    for (name, _, expression) in expressions {
        assert_int4_ddl(ddl, name, expression);
    }
}

/// Accepts exact int4 results but rejects custom qualified aliases, uncertain widths, and non-int4 result types.
#[test]
fn structure_sql_preserves_int4_casts_and_rejects_ambiguous_source_widths() {
    let structure = r#"CREATE TABLE public.counters (
  payload_json json,
  payload_jsonb jsonb,
  counter_text text,
  json_integer integer GENERATED ALWAYS AS ((payload_json ->> 'counter'::text)::integer) STORED,
  catalog_int4 pg_catalog.int4 GENERATED ALWAYS AS (CAST((payload_jsonb ->> 'counter'::text) AS integer)) STORED,
  uppercase_catalog_int4 PG_CATALOG.int4 GENERATED ALWAYS AS ((payload_jsonb ->> 'counter'::text)::int4) STORED,
  jsonb_int4 int4 GENERATED ALWAYS AS (CAST((payload_jsonb ->> 'counter'::text) AS int4)) STORED,
  text_int4 integer GENERATED ALWAYS AS (counter_text::int4) STORED
);"#;
    assert_eq!(
        structure.matches("counter_text text").count(),
        1,
        "structure fixture declares its text operand once"
    );
    let schema = ingest_structure_sql_with_generated_expression_dialect(
        structure.as_bytes(),
        "db/structure.sql",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("structure.sql exact int4 casts should ingest");
    let counters = table(&schema, "counters");
    for (name, expression) in [
        ("json_integer", "(payload_json ->> 'counter'::text)::integer"),
        (
            "catalog_int4",
            "CAST((payload_jsonb ->> 'counter'::text) AS integer)",
        ),
        (
            "uppercase_catalog_int4",
            "(payload_jsonb ->> 'counter'::text)::int4",
        ),
        (
            "jsonb_int4",
            "CAST((payload_jsonb ->> 'counter'::text) AS int4)",
        ),
        ("text_int4", "counter_text::int4"),
    ] {
        assert_eq!(generated_expression(counters, name), expression);
        assert_eq!(column(counters, name).col_type, ColumnType::Integer);
        assert_eq!(column(counters, name).generated_int4_compatible, None);
    }
    let postgres = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("PostgreSQL structure DDL");
    assert_int4_ddl(
        &postgres[0],
        "json_integer",
        "(payload_json ->> 'counter'::text)::integer",
    );
    assert_int4_ddl(
        &postgres[0],
        "jsonb_int4",
        "CAST((payload_jsonb ->> 'counter'::text) AS int4)",
    );
    for (name, expression) in [
        (
            "catalog_int4",
            "CAST((payload_jsonb ->> 'counter'::text) AS integer)",
        ),
        (
            "uppercase_catalog_int4",
            "(payload_jsonb ->> 'counter'::text)::int4",
        ),
    ] {
        assert_int4_ddl(&postgres[0], name, expression);
    }

    // These source-only cases model custom domains whose names happen to
    // match built-in aliases. `public.integer` and `other_schema.int` may be
    // declared outside this dump. They prove provenance handling and never
    // enter the positive PostgreSQL DDL oracle.
    let qualified_domain_cases = [
        (
            "public.int4",
            "public_int4_result",
            "CREATE DOMAIN public.int4 AS integer CHECK (VALUE >= 0);",
        ),
        ("public.integer", "public_integer_result", ""),
        ("other_schema.int", "other_schema_int_result", ""),
        // SQL aliases integer/int resolve only when unqualified. The catalog
        // contains int4, so these qualified spellings do not prove int4.
        ("pg_catalog.integer", "catalog_integer_result", ""),
        ("pg_catalog.int", "catalog_int_result", ""),
    ];
    for (qualified_type, output_name, declarations) in qualified_domain_cases {
        let source = format!(
            "{declarations}\nCREATE TABLE public.qualified_result (payload jsonb, {output_name} {qualified_type} GENERATED ALWAYS AS ((payload ->> 'counter'::text)::integer) STORED);"
        );
        let error = ingest_structure_sql_with_generated_expression_dialect(
            source.as_bytes(),
            "db/structure.sql",
            GeneratedExpressionDialect::Postgres,
        )
        .expect_err("custom-schema integer aliases must not normalize to built-in int4")
        .to_string();
        assert!(error.contains(output_name), "{qualified_type}: {error}");
        assert!(
            error.contains("not an exact PostgreSQL int4"),
            "{qualified_type}: {error}"
        );
    }

    // Negative-only source fixture: PostgreSQL type modifiers on integer
    // aliases are not emitted by a server dump, but the ingester must retain
    // their uncertainty if malformed/unsupported source text contains them.
    let widths = r#"CREATE TABLE public.integer_provenance (
  bare_integer integer,
  bare_int int,
  bare_int4 int4,
  catalog_integer pg_catalog.integer,
  catalog_int pg_catalog.int,
  catalog_int4 pg_catalog.int4,
  uppercase_catalog_int4 PG_CATALOG.int4,
  small_value smallint,
  int2_value int2,
  serial_value serial,
  serial4_value serial4,
  integer_width integer(4),
  int4_width int4(4),
  big_value bigint,
  bigserial_value bigserial,
  float_value double precision,
  decimal_value numeric
);"#;
    let widths_schema = ingest_structure_sql_with_generated_expression_dialect(
        widths.as_bytes(),
        "db/structure.sql",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("ordinary type provenance schema should ingest");
    let widths_table = table(&widths_schema, "integer_provenance");
    for name in [
        "bare_integer",
        "bare_int",
        "bare_int4",
        "catalog_int4",
        "uppercase_catalog_int4",
    ] {
        assert_eq!(column(widths_table, name).col_type, ColumnType::Integer);
        assert_eq!(column(widths_table, name).generated_int4_compatible, None);
    }
    for name in [
        "catalog_integer",
        "catalog_int",
        "small_value",
        "int2_value",
        "serial_value",
        "serial4_value",
        "integer_width",
        "int4_width",
    ] {
        assert_eq!(column(widths_table, name).col_type, ColumnType::Integer, "{name}");
        assert_eq!(
            column(widths_table, name).generated_int4_compatible,
            Some(false),
            "{name} must retain source-width/alias uncertainty"
        );
    }
    assert_eq!(column(widths_table, "big_value").col_type, ColumnType::BigInt);
    assert_eq!(column(widths_table, "bigserial_value").col_type, ColumnType::BigInt);
    assert_eq!(column(widths_table, "float_value").col_type, ColumnType::Float);
    assert!(matches!(column(widths_table, "decimal_value").col_type, ColumnType::Decimal { .. }));

    // These are rejection-only spellings; in particular serial aliases
    // expand to DEFAULT/sequence behavior and are not applied to PostgreSQL.
    for result_type in [
        "smallint",
        "int2",
        "serial",
        "serial4",
        "bigint",
        "bigserial",
        "numeric",
        "double precision",
    ] {
        let invalid = format!(
            "CREATE TABLE public.invalid_result (payload jsonb, computed {result_type} GENERATED ALWAYS AS ((payload ->> 'counter'::text)::integer) STORED);"
        );
        let error = ingest_structure_sql_with_generated_expression_dialect(
            invalid.as_bytes(),
            "db/structure.sql",
            GeneratedExpressionDialect::Postgres,
        )
        .expect_err("only exact int4 result types are supported")
        .to_string();
        assert!(error.contains("computed"), "{result_type}: {error}");
        let expected_reason = if matches!(result_type, "smallint" | "int2" | "serial" | "serial4") {
            "not an exact PostgreSQL int4"
        } else {
            "supported generated-column result types"
        };
        assert!(
            error.contains(expected_reason),
            "{result_type}: {error}"
        );
    }

    let portable = ingest_structure_sql(structure.as_bytes(), "db/structure.sql")
        .expect_err("ordinary structure SQL remains Portable")
        .to_string();
    assert!(portable.contains("json_integer"), "{portable}");
}

/// Pairs accepted text-to-int4 shapes with negative controls for wrong operands, targets, result declarations, and references.
#[test]
fn int4_parser_accepts_only_text_casts_with_postgres_integer_targets() {
    let accepted = [
        ("postfix_integer", "(payload_jsonb ->> 'counter'::text)::integer"),
        ("postfix_int4", "(payload_jsonb ->> 'counter'::text)::int4"),
        (
            "cast_integer",
            "CAST((payload_jsonb ->> 'counter'::text) AS integer)",
        ),
        (
            "cast_int4",
            "CAST((payload_jsonb ->> 'counter'::text) AS int4)",
        ),
        ("text_source", "counter_text::integer"),
        ("parenthesized", "((payload_jsonb ->> ('counter'::text)))::int4"),
    ];
    let source = schema_source(
        &[],
        &accepted.map(|(name, expression)| (name, ":integer", expression)),
    );
    let schema = ingest_postgres(&source).expect("whitelisted text-to-int4 forms");
    for (name, expression) in accepted {
        assert_eq!(generated_expression(table(&schema, "counters"), name), expression);
    }

    for expression in [
        "payload_jsonb ->> 'counter'::text",
        "payload_jsonb ->> 'counter'::integer",
        "payload_jsonb ->> 'counter'::text::integer",
        "payload_jsonb ->> 'counter'::text::int4",
        "payload_jsonb::integer",
        "CAST(payload_jsonb AS integer)",
        "(payload_jsonb ->> 'counter'::text)::smallint",
        "(payload_jsonb ->> 'counter'::text)::int2",
        "(payload_jsonb ->> 'counter'::text)::bigint",
        "(payload_jsonb ->> 'counter'::text)::int8",
        "(payload_jsonb ->> 'counter'::text)::int",
        "(payload_jsonb ->> 'counter'::text)::numeric",
        "(payload_jsonb ->> 'counter'::text)::float",
        "CAST((payload_jsonb ->> 'counter'::text) AS smallint)",
        "coalesce(payload_jsonb ->> 'counter'::text, 0)::integer",
        "missing_counter::integer",
    ] {
        let error = pg_error(&schema_source(
            &[],
            &[("invalid_counter", ":integer", expression)],
        ));
        assert!(error.contains("invalid_counter"), "{expression}: {error}");
    }

    let text_output = pg_error(&schema_source(
        &[],
        &[(
            "text_output",
            ":string",
            "(payload_jsonb ->> 'counter'::text)::integer",
        )],
    ));
    assert!(text_output.contains("text_output"), "{text_output}");

    let generated_reference = schema_source(
        &[],
        &[
            ("seed", ":string", "payload_jsonb ->> 'label'::text"),
            ("derived_int", ":integer", "seed::integer"),
        ],
    );
    let reference_error = pg_error(&generated_reference);
    assert!(reference_error.contains("generated column `seed`"), "{reference_error}");

    for (type_options, name) in [
        (":integer, limit: 0", "limit_zero"),
        (":integer, limit: 1", "limit_one"),
        (":integer, limit: 2", "limit_two"),
        (":integer, limit: 9", "limit_nine"),
        (":integer, limit: 5", "limit_bigint"),
        (":bigint", "bigint"),
        (":float", "float"),
        (":decimal", "decimal"),
    ] {
        let error = pg_error(&schema_source(
            &[],
            &[(name, type_options, INT4_EXPRESSION)],
        ));
        assert!(error.contains(name), "{type_options}: {error}");
    }
}

/// Confirms int4 casts remain unavailable to normal schema, application, project, and SQLite paths.
#[test]
fn portable_app_project_and_sqlite_paths_reject_generated_int4_casts() {
    let source = schema_source(&[], &[("generated_counter", ":integer", INT4_EXPRESSION)]);
    let assert_portable_result_type_rejection = |path: &str, error: &str| {
        assert!(
            error.contains("counters.generated_counter"),
            "{path} should identify the rejected generated column: {error}"
        );
        assert!(
            error.contains(
                "the supported generated-column result types are unbounded string and text"
            ),
            "{path} should report the Portable result-type boundary: {error}"
        );
    };

    let schema_error = ingest_schema(source.as_bytes(), "db/schema.rb")
        .expect_err("normal schema ingest remains Portable")
        .to_string();
    assert_portable_result_type_rejection("schema ingest", &schema_error);

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
        .expect_err("normal app ingest remains Portable")
        .to_string();
    assert_portable_result_type_rejection("app ingest", &app_error);

    let schema = ingest_postgres(&source).expect("explicit PostgreSQL mode accepts int4");
    let sqlite_error = render_schema_statements_for(&schema, Dialect::Sqlite)
        .expect_err("SQLite DDL rejects PostgreSQL int4 casts")
        .to_string();
    assert_portable_result_type_rejection("SQLite DDL", &sqlite_error);

    let mut app = roundhouse::App::new();
    app.schema = schema;
    let project_error = target_files(&app, Path::new("."), BuildTarget::Ruby)
        .expect_err("project generation remains gated by SQLite support");
    assert_portable_result_type_rejection("project generation", &project_error);
}

/// Checks migration preservation, atomic rejection of invalid int4 candidates, and survey recovery to the last valid schema.
#[test]
fn migrations_preserve_int4_sql_and_reject_invalid_candidate_mutations_atomically() {
    let base_source = schema_source(&[], &[]);
    let mut schema = ingest_postgres(&base_source).expect("base json/text schema");
    let migration = r#"class AddGeneratedCounts < ActiveRecord::Migration[8.1]
  def change
    add_column :counters, :json_count, :integer, as: "(payload_json ->> 'counter'::text)::integer", stored: true
    add_column :counters, :jsonb_count, :integer, as: "CAST((payload_jsonb ->> 'counter'::text) AS int4)", stored: true
    add_column :counters, :text_count, :integer, as: "counter_text::int4", stored: true
  end
end
"#;
    ingest_migration_with_generated_expression_dialect(
        migration.as_bytes(),
        "db/migrate/add_generated_counts.rb",
        &mut schema,
        GeneratedExpressionDialect::Postgres,
    )
    .expect("PostgreSQL migration fold accepts bounded int4 outputs");
    let counters = table(&schema, "counters");
    for (name, expression) in [
        ("json_count", "(payload_json ->> 'counter'::text)::integer"),
        (
            "jsonb_count",
            "CAST((payload_jsonb ->> 'counter'::text) AS int4)",
        ),
        ("text_count", "counter_text::int4"),
    ] {
        assert_eq!(generated_expression(counters, name), expression);
        assert_eq!(column(counters, name).col_type, ColumnType::Integer);
        assert_eq!(column(counters, name).generated_int4_compatible, None);
    }

    let mut portable_schema = ingest_schema(base_source.as_bytes(), "db/schema.rb")
        .expect("ordinary JSON/text schema remains portable");
    let portable_error = ingest_migration(
        migration.as_bytes(),
        "db/migrate/add_generated_counts.rb",
        &mut portable_schema,
    )
    .expect_err("ordinary migration wrapper remains Portable")
    .to_string();
    assert!(portable_error.contains("json_count"), "{portable_error}");

    let mut candidate = ingest_postgres(&schema_source(
        &[],
        &[("generated_count", ":integer", INT4_EXPRESSION)],
    ))
    .expect("valid generated int4 base");
    let original = candidate.clone();
    let invalidation = r#"class ChangePayloadType < ActiveRecord::Migration[8.1]
  def change
    change_column :counters, :payload_jsonb, :text
  end
end
"#;
    let error = ingest_migration_with_generated_expression_dialect(
        invalidation.as_bytes(),
        "db/migrate/change_payload_type.rb",
        &mut candidate,
        GeneratedExpressionDialect::Postgres,
    )
    .expect_err("changing the JSON operand type must reject the candidate")
    .to_string();
    assert!(error.contains("generated column mutation"), "{error}");
    assert_eq!(candidate, original, "strict candidate rejection is atomic");

    let mut surveyed = original.clone();
    survey::activate();
    let result = survey::unwrap_or_record(ingest_migration_with_generated_expression_dialect(
        invalidation.as_bytes(),
        "db/migrate/change_payload_type.rb",
        &mut surveyed,
        GeneratedExpressionDialect::Postgres,
    ));
    let gaps = survey::drain();
    assert!(matches!(result, Ok(None)), "survey skips only the rejected operation: {result:?}");
    assert_eq!(surveyed, original, "survey recovery retains the last valid schema");
    assert_eq!(gaps.len(), 1, "one rejected operation is ledgered once: {gaps:?}");
    assert!(gaps[0].to_string().contains("change_column"), "{gaps:?}");
}

/// Emits marked PostgreSQL DDL and oracle SQL for live checks of exact integer outputs and source updates.
#[test]
fn postgres_generated_int4_ddl_oracle_fixture_emits_marked_sql() {
    let expressions = [
        ("from_json", ":integer", "(payload_json ->> 'counter'::text)::integer"),
        (
            "from_jsonb",
            ":integer",
            "CAST((payload_jsonb ->> 'counter'::text) AS int4)",
        ),
        ("from_text", ":integer", "counter_text::integer"),
    ];
    let source = schema_source(
        &["t.string \"case_name\", null: false"],
        &expressions,
    );
    let source = source.replace(
        "  end\nend\n",
        concat!(
            "  end\n",
            "  create_table \"required_counters\", force: :cascade do |t|\n",
            "    t.jsonb \"payload_jsonb\"\n",
            "    t.virtual \"required_count\", type: :integer, as: \"(payload_jsonb ->> 'counter'::text)::integer\", stored: true, null: false\n",
            "  end\nend\n"
        ),
    );
    let schema = ingest_postgres(&source).expect("oracle DDL fixture schema");
    let statements = render_schema_statements_for(&schema, Dialect::Postgres)
        .expect("oracle DDL fixture renders");
    assert_eq!(statements.len(), 2);
    let main_ddl = statements
        .iter()
        .find(|ddl| ddl.starts_with("CREATE TABLE IF NOT EXISTS \"counters\""))
        .expect("counters DDL");
    let required_ddl = statements
        .iter()
        .find(|ddl| ddl.starts_with("CREATE TABLE IF NOT EXISTS \"required_counters\""))
        .expect("required_counters DDL");
    assert!(main_ddl.contains("\"payload_json\" json,"), "{main_ddl}");
    assert!(main_ddl.contains("\"payload_jsonb\" jsonb,"), "{main_ddl}");
    for (name, _, expression) in expressions {
        assert_int4_ddl(main_ddl, name, expression);
    }
    assert!(required_ddl.contains("\"required_count\" integer GENERATED ALWAYS AS ((payload_jsonb ->> 'counter'::text)::integer) STORED NOT NULL"), "{required_ddl}");
    assert_eq!(
        main_ddl.matches("\"counter_text\" text").count(),
        1,
        "emitted counters DDL declares its text operand once"
    );
    assert!(statements.iter().all(|statement| !statement.trim_end().ends_with(';')));

    let emitted_statements = statements
        .iter()
        .map(|statement| format!("{statement};"))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(emitted_statements.matches("CREATE TABLE IF NOT EXISTS").count(), 2);
    assert_eq!(emitted_statements.matches(");").count(), 2);
    // With --nocapture this emits both blocks needed for a live database check.
    // On a disposable PostgreSQL database, run BEGIN, the marked DDL, the
    // marked oracle SQL, then ROLLBACK, using psql -X -v ON_ERROR_STOP=1.
    println!("{DDL_BEGIN}\n{emitted_statements}\n{DDL_END}");
    println!("ROUNDHOUSE_POSTGRES_GENERATED_INT4_ORACLE_SQL_BEGIN");
    print!("{}", include_str!("support/postgres_generated_integers_oracle.sql"));
    println!("ROUNDHOUSE_POSTGRES_GENERATED_INT4_ORACLE_SQL_END");
}
