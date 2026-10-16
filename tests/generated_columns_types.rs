//! Generated-expression validation needs source-type provenance when
//! ordinary schema typing normalizes database-specific text-like types.

use roundhouse::ingest::structure_sql::{
    ingest_structure_sql, ingest_structure_sql_with_generated_expression_dialect,
};
use roundhouse::ingest::{ingest_migration, ingest_schema};
use roundhouse::schema::generated::GeneratedExpressionDialect;
use roundhouse::schema::Schema;

fn schema_error(source: &str) -> String {
    ingest_schema(source.as_bytes(), "db/schema.rb")
        .expect_err("non-portable source types must not pass generated-expression validation")
        .to_string()
}

/// Finds a column in schema IR so tests can inspect ordinary typing and retained source provenance together.
fn structure_column<'a>(
    schema: &'a Schema,
    name: &str,
) -> &'a roundhouse::schema::Column {
    schema.tables[&roundhouse::Symbol::from("people")]
        .columns
        .iter()
        .find(|column| column.name.as_str() == name)
        .expect("column is ingested for ordinary typing")
}

#[test]
fn schema_rb_rejects_nonportable_result_and_operand_types() {
    let operand = schema_error(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.inet "remote_ip"
    t.string "first_name"
    t.virtual "display", type: :string, as: "remote_ip || 'x'", stored: true
  end
end
"#,
    );
    assert!(operand.contains("non-portable text semantics"), "{operand}");

    let result = schema_error(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "display", type: :inet, as: "first_name || 'x'", stored: true
  end
end
"#,
    );
    assert!(
        result.contains("original generated-column result type"),
        "{result}"
    );

    let enum_operand = schema_error(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.enum "status"
    t.virtual "label", type: :text, as: "status || 'x'", stored: true
  end
end
"#,
    );
    assert!(
        enum_operand.contains("non-portable text semantics"),
        "{enum_operand}"
    );
}

#[test]
fn schema_rb_rejects_text_limits_that_normalization_would_discard() {
    let operand = schema_error(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.text "source_text", limit: 32
    t.virtual "display", type: :string, as: "source_text || 'x'", stored: true
  end
end
"#,
    );
    assert!(operand.contains("non-portable text semantics"), "{operand}");

    let result = schema_error(
        r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.virtual "display", type: :text, as: "first_name || 'x'", limit: 32, stored: true
  end
end
"#,
    );
    assert!(
        result.contains("original generated-column result type"),
        "{result}"
    );
}

#[test]
fn migration_folds_retain_nonportable_type_provenance() {
    let create_inet = r#"class CreatePeople < ActiveRecord::Migration[8.1]
  def change
    create_table :people do |t|
      t.inet :remote_ip
    end
  end
end
"#;
    let add_generated = r#"class AddDisplay < ActiveRecord::Migration[8.1]
  def change
    add_column :people, :display, :string, as: "remote_ip || 'x'", stored: true
  end
end
"#;
    let mut schema = Schema::default();
    ingest_migration(create_inet.as_bytes(), "001_create_people.rb", &mut schema)
        .expect("ordinary migration column");
    let inet = ingest_migration(add_generated.as_bytes(), "002_add_display.rb", &mut schema)
        .expect_err("migration folds must preserve source type evidence");
    assert!(
        inet.to_string().contains("non-portable text semantics"),
        "{inet}"
    );

    let create_enum = r#"class CreatePeople < ActiveRecord::Migration[8.1]
  def change
    create_table :people do |t|
      t.enum :status
    end
  end
end
"#;
    let add_enum_generated = r#"class AddLabel < ActiveRecord::Migration[8.1]
  def change
    add_column :people, :label, :text, as: "status || 'x'", stored: true
  end
end
"#;
    let mut schema = Schema::default();
    ingest_migration(create_enum.as_bytes(), "001_create_people.rb", &mut schema)
        .expect("ordinary enum migration column");
    let enum_error = ingest_migration(
        add_enum_generated.as_bytes(),
        "002_add_label.rb",
        &mut schema,
    )
    .expect_err("enum provenance must survive across migration files");
    assert!(
        enum_error
            .to_string()
            .contains("non-portable text semantics"),
        "{enum_error}"
    );
}

#[test]
fn structure_sql_rejects_text_like_aliases_but_accepts_unbounded_varchar() {
    for source_type in [
        "inet",
        "cidr",
        "macaddr",
        "macaddr8",
        "interval",
        "character",
    ] {
        let dump = format!(
            "CREATE TABLE public.people (source_text {source_type}, label text GENERATED ALWAYS AS (source_text || 'x') STORED);"
        );
        let error = ingest_structure_sql(dump.as_bytes(), "db/structure.sql")
            .expect_err("collapsed aliases must not become supported generated operands")
            .to_string();
        assert!(
            error.contains("non-portable text semantics"),
            "{source_type}: {error}"
        );
    }

    let enum_dump = r#"CREATE TYPE public.widget_status AS ENUM ('draft', 'published');
CREATE TABLE public.people (
  status_code public.widget_status,
  label text GENERATED ALWAYS AS (status_code || 'x') STORED
);"#;
    let enum_error = ingest_structure_sql(enum_dump.as_bytes(), "db/structure.sql")
        .expect_err("registered Postgres enums normalize to text but are not text operands")
        .to_string();
    assert!(
        enum_error.contains("non-portable text semantics"),
        "{enum_error}"
    );

    let portable = r#"CREATE TABLE public.people (
  source_text character varying,
  label text GENERATED ALWAYS AS (source_text || 'x') STORED
);"#;
    let schema = ingest_structure_sql(portable.as_bytes(), "db/structure.sql")
        .expect("unbounded varying text is a supported operand");
    let people = &schema.tables[&roundhouse::Symbol::from("people")];
    assert_eq!(people.columns.len(), 2);
}

#[test]
fn structure_sql_text_typmods_are_not_treated_as_plain_unbounded_text() {
    let dump = r#"CREATE TABLE public.people (
  source_text text(12),
  label text GENERATED ALWAYS AS (source_text || 'x') STORED
);"#;
    let error = ingest_structure_sql(dump.as_bytes(), "db/structure.sql")
        .expect_err("a discarded text typmod must not pass the generated text boundary")
        .to_string();
    assert!(error.contains("non-portable text semantics"), "{error}");
}

/// Shows that PostgreSQL casts do not make domains, non-text types, or bounded text sources valid operands.
#[test]
fn explicit_postgres_mode_rejects_nonportable_and_bounded_text_sources() {
    let cases = [
        (
            "inet",
            "CREATE TABLE public.people (source_text inet, label text GENERATED ALWAYS AS (source_text::text) STORED);",
            "non-portable text semantics",
        ),
        (
            "citext",
            "CREATE TABLE public.people (source_text citext, label text GENERATED ALWAYS AS (source_text::text) STORED);",
            "non-portable text semantics",
        ),
        (
            "fixed character",
            "CREATE TABLE public.people (source_text character(12), label text GENERATED ALWAYS AS (source_text::text) STORED);",
            "non-portable text semantics",
        ),
        (
            "bounded varchar",
            "CREATE TABLE public.people (source_text varchar(12), label text GENERATED ALWAYS AS (source_text::text) STORED);",
            "String { limit: Some(12) }",
        ),
        (
            "text typmod",
            "CREATE TABLE public.people (source_text text(12), label text GENERATED ALWAYS AS (source_text::text) STORED);",
            "non-portable text semantics",
        ),
    ];

    for (source_type, dump, expected_reason) in cases {
        let error = ingest_structure_sql_with_generated_expression_dialect(
            dump.as_bytes(),
            "db/structure.sql",
            GeneratedExpressionDialect::Postgres,
        )
        .expect_err("explicit PostgreSQL casts do not widen the text-source type boundary")
        .to_string();
        assert!(
            error.contains(expected_reason),
            "{source_type}: expected {expected_reason:?}, got {error}"
        );
    }

    let enum_dump = r#"CREATE TYPE public.widget_status AS ENUM ('draft', 'published');
CREATE TABLE public.people (
  status_code public.widget_status,
  label text GENERATED ALWAYS AS (status_code::text) STORED
);"#;
    let enum_error = ingest_structure_sql_with_generated_expression_dialect(
        enum_dump.as_bytes(),
        "db/structure.sql",
        GeneratedExpressionDialect::Postgres,
    )
    .expect_err("Postgres enum provenance must remain non-portable in explicit mode")
    .to_string();
    assert!(
        enum_error.contains("non-portable text semantics"),
        "{enum_error}"
    );

    let unbounded = r#"CREATE TABLE public.people (
  source_text character varying,
  label text GENERATED ALWAYS AS (source_text::text) STORED
);"#;
    ingest_structure_sql_with_generated_expression_dialect(
        unbounded.as_bytes(),
        "db/structure.sql",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("unbounded character varying remains an eligible text operand");
}

/// Separates ordinary type mapping from generated-expression proof: custom qualified aliases keep their app type but fail as exact sources or outputs.
#[test]
fn structure_sql_keeps_ordinary_qualified_types_but_guards_generated_provenance() {
    let ordinary = r#"CREATE TABLE public.people (
  bare_text text,
  bare_varchar varchar,
  catalog_text pg_catalog.text,
  catalog_varchar pg_catalog.varchar,
  custom_text public.text,
  custom_varchar public.varchar
);"#;
    let schema = ingest_structure_sql(ordinary.as_bytes(), "db/structure.sql")
        .expect("ordinary typing keeps the existing normalized built-in mappings");
    for name in ["bare_text", "catalog_text", "custom_text"] {
        assert!(
            matches!(
                &structure_column(&schema, name).col_type,
                roundhouse::schema::ColumnType::Text
            ),
            "{name} should remain ordinarily typed as text"
        );
    }
    for name in ["bare_varchar", "catalog_varchar", "custom_varchar"] {
        assert!(
            matches!(
                &structure_column(&schema, name).col_type,
                roundhouse::schema::ColumnType::String { limit: None }
            ),
            "{name} should remain ordinarily typed as unbounded string"
        );
    }

    for output_type in ["public.text", "public.varchar"] {
        let dump = format!(
            "CREATE TABLE public.people (id integer, label {output_type} GENERATED ALWAYS AS ('x') STORED);"
        );
        let error = ingest_structure_sql(dump.as_bytes(), "db/structure.sql")
            .expect_err("a custom qualified output type must not become an exact builtin")
            .to_string();
        assert!(
            error.contains("original generated-column result type"),
            "{output_type}: {error}"
        );
    }

    // Quoted catalog names need case-sensitive SQL handling that the current
    // type mapper does not preserve, so even quoted PG_CATALOG is guarded.
    for source_type in ["public.text", "public.varchar", r#""PG_CATALOG".text"#] {
        let dump = format!(
            "CREATE TABLE public.people (id integer, source_value {source_type}, label text GENERATED ALWAYS AS (source_value || 'x') STORED);"
        );
        let error = ingest_structure_sql(dump.as_bytes(), "db/structure.sql")
            .expect_err("a custom qualified source type must not become an exact builtin")
            .to_string();
        assert!(
            error.contains("non-portable text semantics"),
            "{source_type}: {error}"
        );
    }

    let catalog_builtins = r#"CREATE TABLE public.people (
  id integer,
  source_value pg_catalog.text,
  label pg_catalog.varchar GENERATED ALWAYS AS (source_value || 'x') STORED
);"#;
    ingest_structure_sql(catalog_builtins.as_bytes(), "db/structure.sql")
        .expect("exact pg_catalog built-in names remain eligible");
}

/// Checks that array metadata leaves ordinary mapped types unchanged while only literal false or nil preserves scalar eligibility.
#[test]
fn schema_rb_array_sources_are_guarded_without_changing_scalar_controls() {
    let ordinary = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "source_text", array: true
  end
end
"#;
    let schema = ingest_schema(ordinary.as_bytes(), "db/schema.rb")
        .expect("ordinary array columns retain their existing application type");
    let source = structure_column(&schema, "source_text");
    assert!(
        matches!(
            &source.col_type,
            roundhouse::schema::ColumnType::String { limit: None }
        ),
        "array metadata must not alter the ordinary mapped type"
    );
    assert_eq!(source.generated_text_compatible, Some(false));

    for array_option in ["true", "array_option"] {
        let source = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "source_text", array: {array_option}
    t.virtual "display", type: :string, as: "source_text::text", stored: true
  end
end
"#
        );
        let error = roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
            source.as_bytes(),
            "db/schema.rb",
            GeneratedExpressionDialect::Postgres,
        )
        .expect_err("array or unknown array metadata cannot be used as a scalar source")
        .to_string();
        assert!(
            error.contains("non-portable text semantics"),
            "array: {array_option}: {error}"
        );
    }

    let scalar_controls = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "false_source", array: false
    t.string "nil_source", array: nil
    t.virtual "false_display", type: :string, as: "false_source::text", stored: true
    t.virtual "nil_display", type: :string, as: "nil_source::text", stored: true
  end
end
"#;
    roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
        scalar_controls.as_bytes(),
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("literal false and nil do not declare an array column");
}

/// Requires a scalar-to-array source mutation to fail atomically and retain the original generated column.
#[test]
fn migration_array_replacement_refuses_before_mutating_generated_schema() {
    let create_people = r#"class CreatePeople < ActiveRecord::Migration[8.1]
  def change
    create_table :people do |t|
      t.string :source_text
      t.virtual :display, type: :string, as: "source_text::text", stored: true
    end
  end
end
"#;
    let make_source_an_array = r#"class MakeSourceAnArray < ActiveRecord::Migration[8.1]
  def change
    change_column :people, :source_text, :string, array: true
  end
end
"#;
    let mut schema = Schema::default();
    roundhouse::ingest::ingest_migration_with_generated_expression_dialect(
        create_people.as_bytes(),
        "001_create_people.rb",
        &mut schema,
        GeneratedExpressionDialect::Postgres,
    )
    .expect("initial scalar source and generated expression are valid");

    let error = roundhouse::ingest::ingest_migration_with_generated_expression_dialect(
        make_source_an_array.as_bytes(),
        "002_make_source_an_array.rb",
        &mut schema,
        GeneratedExpressionDialect::Postgres,
    )
    .expect_err("changing a generated-expression source to an array must be refused")
    .to_string();
    assert!(
        error.contains("non-portable text semantics"),
        "{error}"
    );
    assert_eq!(
        structure_column(&schema, "source_text").generated_text_compatible,
        None,
        "a rejected candidate must leave the original scalar source in place"
    );
    assert!(
        structure_column(&schema, "display").generated.is_some(),
        "the original generated column must survive the rejected candidate"
    );
}

/// Keeps ordinary JSON and JSONB column mapping while rejecting custom-qualified domains as extraction sources.
#[test]
fn structure_sql_guards_qualified_json_sources_but_keeps_ordinary_json_typing() {
    let ordinary = r#"CREATE TABLE public.people (
  bare_json json,
  bare_jsonb jsonb,
  catalog_json pg_catalog.json,
  catalog_jsonb pg_catalog.jsonb,
  custom_json public.json,
  custom_jsonb public.jsonb
);"#;
    let schema = ingest_structure_sql(ordinary.as_bytes(), "db/structure.sql")
        .expect("qualified JSON columns remain available for ordinary app typing");
    for name in ["bare_json", "catalog_json", "custom_json"] {
        assert!(
            matches!(
                &structure_column(&schema, name).col_type,
                roundhouse::schema::ColumnType::Json
            ),
            "{name} should remain ordinarily typed as JSON"
        );
    }
    for name in ["bare_jsonb", "catalog_jsonb", "custom_jsonb"] {
        assert!(
            matches!(
                &structure_column(&schema, name).col_type,
                roundhouse::schema::ColumnType::Jsonb
            ),
            "{name} should remain ordinarily typed as JSONB"
        );
    }

    for source_type in ["public.json", "public.jsonb"] {
        let dump = format!(
            "CREATE TABLE public.people (id integer, source_document {source_type}, label text GENERATED ALWAYS AS (source_document ->> 'name') STORED);"
        );
        let error = ingest_structure_sql_with_generated_expression_dialect(
            dump.as_bytes(),
            "db/structure.sql",
            GeneratedExpressionDialect::Postgres,
        )
        .expect_err("JSON extraction must reject a custom qualified JSON domain")
        .to_string();
        assert!(
            error.contains("non-portable text semantics"),
            "{source_type}: {error}"
        );
    }

    for source_type in ["pg_catalog.json", "pg_catalog.jsonb"] {
        let dump = format!(
            "CREATE TABLE public.people (id integer, source_document {source_type}, label text GENERATED ALWAYS AS (source_document ->> 'name') STORED);"
        );
        ingest_structure_sql_with_generated_expression_dialect(
            dump.as_bytes(),
            "db/structure.sql",
            GeneratedExpressionDialect::Postgres,
        )
        .expect("exact pg_catalog JSON source types remain eligible");
    }
}

/// Checks that JSON array columns retain ordinary JSON typing but fail scalar extraction; false and nil remain scalar controls.
#[test]
fn schema_rb_json_arrays_are_not_scalar_json_extraction_sources() {
    let ordinary = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.json "payload_json", array: true
    t.jsonb "payload_jsonb", array: true
  end
end
"#;
    let schema = ingest_schema(ordinary.as_bytes(), "db/schema.rb")
        .expect("JSON array columns retain their existing ordinary application types");
    for (name, expected) in [
        ("payload_json", roundhouse::schema::ColumnType::Json),
        ("payload_jsonb", roundhouse::schema::ColumnType::Jsonb),
    ] {
        assert_eq!(
            &structure_column(&schema, name).col_type,
            &expected,
            "{name} ordinary typing should remain unchanged"
        );
        assert_eq!(
            structure_column(&schema, name).generated_text_compatible,
            Some(false),
            "{name} must retain negative scalar provenance"
        );
    }

    for json_type in ["json", "jsonb"] {
        let source = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.{json_type} "payload", array: true
    t.virtual "name", type: :string, as: "payload ->> 'name'", stored: true
  end
end
"#
        );
        let error = roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
            source.as_bytes(),
            "db/schema.rb",
            GeneratedExpressionDialect::Postgres,
        )
        .expect_err("JSON extraction must reject an array operand")
        .to_string();
        assert!(
            error.contains("non-portable text semantics"),
            "{json_type}: {error}"
        );
    }

    let scalar_controls = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.json "payload_json", array: false
    t.jsonb "payload_jsonb", array: nil
    t.virtual "json_name", type: :string, as: "payload_json ->> 'name'", stored: true
    t.virtual "jsonb_name", type: :string, as: "payload_jsonb ->> 'name'", stored: true
  end
end
"#;
    roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
        scalar_controls.as_bytes(),
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("literal false and nil JSON options remain scalar controls");
}
