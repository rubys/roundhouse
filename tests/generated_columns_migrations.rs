//! Migration folds must preserve schema metadata when a generated-column
//! change is rejected, and must not retain stale generated/index references.

use roundhouse::Symbol;
use roundhouse::emit::shared::schema_sql::render_schema_statements;
use roundhouse::ingest::{ingest_migration, ingest_schema, survey};
use roundhouse::schema::generated::GeneratedExpressionDialect;
use roundhouse::schema::{Column, ColumnType, GeneratedColumnStorage, Schema, Table};

fn generated_schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.string "nickname"
    t.string "obsolete"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || last_name", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("generated-column schema should ingest")
}

fn ordinary_display_schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.string "display_name", default: "legacy", null: false
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ordinary-column schema should ingest")
}

fn indexed_generated_schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || last_name", stored: true
    t.index ["display_name"], name: "index_people_on_display_name"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("indexed generated-column schema should ingest")
}

/// Builds a PostgreSQL-mode schema whose generated output depends on a text cast.
fn postgres_text_cast_schema() -> Schema {
    roundhouse::ingest::ingest_schema_with_generated_expression_dialect(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "nickname"
    t.virtual "display_name", type: :string, as: "first_name::text", stored: true
  end
end
"#,
        "db/schema.rb",
        GeneratedExpressionDialect::Postgres,
    )
    .expect("the PostgreSQL DDL path should ingest a text cast")
}

fn reference_schema() -> Schema {
    ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "name", default: "keep", null: false
    t.references "author"
    t.belongs_to "reviewer"
    t.virtual "display_name", type: :string, as: "\"name\" || ' display'", stored: true
  end
end
"#,
        "db/schema.rb",
    )
    .expect("reference-column schema should ingest")
}

#[test]
fn change_column_default_rejects_generated_columns_atomically() {
    for (form, operation) in [
        (
            "positional",
            r#"change_column_default :people, :display_name, "fallback""#,
        ),
        (
            "from/to keyword",
            r#"change_column_default :people, :display_name, from: nil, to: "fallback""#,
        ),
    ] {
        let mut schema = generated_schema();
        let original = schema.clone();
        let error = apply_migration(&mut schema, &one_operation(operation))
            .expect_err("a generated column cannot be assigned a default");

        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains("default"), "{error}");
        assert_eq!(
            schema, original,
            "the {form} default change must leave the entire schema unchanged"
        );
        let generated = column(table(&schema, "people"), "display_name");
        assert!(generated.default.is_none());
        assert_eq!(
            generated.generated.as_ref().unwrap().expression,
            "first_name || ' ' || last_name"
        );
    }
}

#[test]
fn change_column_default_forms_still_update_ordinary_columns() {
    for (operation, expected_default) in [
        (
            r#"change_column_default :people, :nickname, "public""#,
            "public",
        ),
        (
            r#"change_column_default :people, :nickname, from: nil, to: "public""#,
            "public",
        ),
    ] {
        let mut schema = generated_schema();
        apply_migration(&mut schema, &one_operation(operation))
            .expect("ordinary-column default changes remain supported");

        let people = table(&schema, "people");
        assert_eq!(
            column(people, "nickname").default.as_deref(),
            Some(expected_default)
        );
        assert_eq!(
            column(people, "display_name")
                .generated
                .as_ref()
                .unwrap()
                .expression,
            "first_name || ' ' || last_name"
        );
    }
}

#[test]
fn change_column_null_remains_supported_for_generated_and_ordinary_columns() {
    for target in ["display_name", "nickname"] {
        let mut schema = generated_schema();
        apply_migration(
            &mut schema,
            &one_operation(&format!("change_column_null :people, :{target}, false")),
        )
        .expect("change_column_null remains supported");

        let changed = column(table(&schema, "people"), target);
        assert!(!changed.nullable);
        assert!(changed.default.is_none());
        if target == "display_name" {
            assert!(changed.generated.is_some());
        }
    }
}

#[test]
fn reference_removal_aliases_cannot_remove_generated_outputs_or_stale_indexes() {
    for (verb, operation) in [
        (
            "remove_reference",
            "remove_reference :people, :display_name",
        ),
        (
            "remove_belongs_to",
            "remove_belongs_to :people, :display_name",
        ),
    ] {
        for (kind, mut schema) in [
            ("unindexed", generated_schema()),
            ("indexed", indexed_generated_schema()),
        ] {
            let original = schema.clone();
            let error = apply_migration(&mut schema, &one_operation(operation))
                .expect_err("a reference-removal alias cannot remove a generated output");

            assert!(error.contains("generated column"), "{error}");
            assert_eq!(
                schema, original,
                "{verb} must preserve the {kind} generated output and its table metadata"
            );
        }
    }
}

#[test]
fn reference_removal_aliases_cannot_remove_and_recreate_generated_operands() {
    for (verb, operations) in [
        (
            "remove_reference",
            "remove_reference :people, :first_name\n    add_column :people, :first_name, :string",
        ),
        (
            "remove_belongs_to",
            "remove_belongs_to :people, :first_name\n    add_column :people, :first_name, :string",
        ),
    ] {
        let mut schema = generated_schema();
        let original = schema.clone();
        let error = apply_migration(&mut schema, &one_operation(operations))
            .expect_err("removing a generated expression operand must be rejected immediately");

        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains(verb), "{error}");
        assert_eq!(
            schema, original,
            "{verb} followed by a same-named replacement must leave the schema unchanged"
        );
        assert_eq!(
            column(table(&schema, "people"), "first_name").col_type,
            ColumnType::String { limit: None }
        );
    }
}

#[test]
fn reference_removal_aliases_still_remove_ordinary_references() {
    for (verb, target) in [
        ("remove_reference", "author"),
        ("remove_belongs_to", "reviewer"),
    ] {
        let mut schema = reference_schema();
        let before = table(&schema, "people");
        assert!(matches!(
            &column(before, target).col_type,
            ColumnType::Reference { .. }
        ));

        apply_migration(
            &mut schema,
            &one_operation(&format!("{verb} :people, :{target}")),
        )
        .expect("ordinary reference columns remain removable");

        let people = table(&schema, "people");
        assert!(
            people
                .columns
                .iter()
                .all(|column| column.name.as_str() != target)
        );
        assert_eq!(column(people, "name").default.as_deref(), Some("keep"));
        assert!(!column(people, "name").nullable);
        let other_reference = if target == "author" {
            "reviewer"
        } else {
            "author"
        };
        assert!(matches!(
            &column(people, other_reference).col_type,
            ColumnType::Reference { .. }
        ));
        let generated = column(people, "display_name")
            .generated
            .as_ref()
            .expect("the unrelated generated output remains present");
        assert_eq!(generated.expression, r#""name" || ' display'"#);
    }
}

/// Applies one migration through the default Portable expression-validation path.
fn apply_migration(schema: &mut Schema, body: &str) -> Result<(), String> {
    ingest_migration(body.as_bytes(), "db/migrate/change_people.rb", schema)
        .map_err(|error| error.to_string())
}

/// Folds a migration through the same explicit expression dialect used to validate PostgreSQL schema DDL.
fn apply_postgres_migration(schema: &mut Schema, body: &str) -> Result<(), String> {
    roundhouse::ingest::ingest_migration_with_generated_expression_dialect(
        body.as_bytes(),
        "db/migrate/change_people.rb",
        schema,
        GeneratedExpressionDialect::Postgres,
    )
    .map_err(|error| error.to_string())
}

/// Finds a named table so migration tests can compare the resulting schema state.
fn table<'a>(schema: &'a Schema, name: &str) -> &'a Table {
    schema
        .tables
        .get(&Symbol::from(name))
        .unwrap_or_else(|| panic!("missing table {name}"))
}

/// Finds a named column for migration-state assertions.
fn column<'a>(table: &'a Table, name: &str) -> &'a Column {
    table
        .columns
        .iter()
        .find(|column| column.name.as_str() == name)
        .unwrap_or_else(|| panic!("missing column {name} on {}", table.name.as_str()))
}

/// Wraps a migration operation in the minimal Rails class needed by the ingester.
fn one_operation(operation: &str) -> String {
    format!(
        "class ChangePeople < ActiveRecord::Migration[8.1]\n  def change\n    {operation}\n  end\nend\n"
    )
}

#[test]
fn invalid_generated_replacements_leave_the_original_table_unchanged() {
    for (mut schema, old_kind) in [
        (generated_schema(), "generated"),
        (ordinary_display_schema(), "ordinary"),
    ] {
        let original = schema.clone();
        let migration = one_operation(
            r#"add_column :people, :display_name, :string, as: "missing_column || ' replacement'", stored: true"#,
        );
        let error = apply_migration(&mut schema, &migration)
            .expect_err("invalid generated replacement must be rejected");
        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains("people.display_name"), "{error}");
        assert_eq!(
            schema, original,
            "the {old_kind} field and full table metadata must survive"
        );

        let existing = column(table(&schema, "people"), "display_name");
        if old_kind == "generated" {
            let generated = existing
                .generated
                .as_ref()
                .expect("original generated field");
            assert_eq!(generated.expression, "first_name || ' ' || last_name");
            assert_eq!(generated.storage, GeneratedColumnStorage::Stored);
        } else {
            assert!(existing.generated.is_none());
            assert_eq!(existing.default.as_deref(), Some("legacy"));
            assert!(!existing.nullable);
        }
    }
}

#[test]
fn source_rename_or_removal_that_invalidates_an_expression_is_atomic() {
    let migrations = [
        (
            "rename_column",
            one_operation(
                "rename_column :people, :first_name, :given_name\n    add_column :people, :first_name, :string",
            ),
        ),
        (
            "remove_column",
            one_operation(
                "remove_column :people, :first_name\n    add_column :people, :first_name, :string",
            ),
        ),
    ];

    for (verb, migration) in migrations {
        let mut schema = generated_schema();
        let original = schema.clone();
        let error = apply_migration(&mut schema, &migration)
            .expect_err("a source-column change must not retarget a generated expression");
        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains(verb), "{error}");
        assert!(error.contains("people.display_name"), "{error}");
        assert!(error.contains("unknown column"), "{error}");
        assert!(error.contains("first_name"), "{error}");
        assert_eq!(
            schema, original,
            "rejected migration must keep original table and SQL"
        );
    }
}

/// Requires source type or name changes that invalidate a cast to leave the prior schema intact; nullability-only changes remain valid.
#[test]
fn postgres_cast_operand_mutations_are_atomic_for_every_candidate_fold() {
    let migrations = [
        (
            "add_column",
            "add_column :people, :first_name, :integer",
            "type Integer",
        ),
        (
            "change_column",
            "change_column :people, :first_name, :integer",
            "type Integer",
        ),
        (
            "remove_column",
            "remove_column :people, :first_name",
            "unknown column `first_name`",
        ),
        (
            "rename_column",
            "rename_column :people, :first_name, :given_name",
            "unknown column `first_name`",
        ),
    ];

    for (verb, operation, reason) in migrations {
        let mut schema = postgres_text_cast_schema();
        let original = schema.clone();
        let error = apply_postgres_migration(&mut schema, &one_operation(operation))
            .expect_err("a cast operand type/name change must be rejected atomically");
        assert!(error.contains("generated column"), "{verb}: {error}");
        assert!(error.contains(reason), "{verb}: {error}");
        assert_eq!(schema, original, "{verb} must leave the prior schema intact");
    }

    let mut schema = postgres_text_cast_schema();
    apply_postgres_migration(
        &mut schema,
        &one_operation("change_column_null :people, :first_name, false"),
    )
    .expect("a nullability-only migration remains supported");
    assert_eq!(
        column(table(&schema, "people"), "display_name")
            .generated
            .as_ref()
            .unwrap()
            .expression,
        "first_name::text"
    );
}

/// Checks that unrelated candidate folds preserve both the cast expression and its PostgreSQL validation mode.
#[test]
fn valid_candidate_folds_keep_the_explicit_postgres_expression_dialect() {
    for operation in [
        "add_column :people, :notes, :string",
        "change_column_default :people, :first_name, \"legacy\"",
        "remove_column :people, :nickname",
        "rename_column :people, :nickname, :handle",
    ] {
        let mut schema = postgres_text_cast_schema();
        apply_postgres_migration(&mut schema, &one_operation(operation))
            .unwrap_or_else(|error| panic!("{operation} should preserve the PG cast: {error}"));
        assert_eq!(
            column(table(&schema, "people"), "display_name")
                .generated
                .as_ref()
                .unwrap()
                .expression,
            "first_name::text",
            "{operation} must retain the source expression"
        );
    }
}

/// Compares Portable migration rejection with successful opt-in PostgreSQL migration ingestion.
#[test]
fn migration_created_text_casts_require_the_explicit_postgres_mode() {
    let migration = one_operation(
        r#"create_table :people do |t|
      t.string :first_name
      t.virtual :normalized_name, type: :string, as: "first_name::text", stored: true
    end"#,
    );

    let mut portable = Schema::default();
    let error = ingest_migration(&migration.as_bytes(), "db/migrate/create_people.rb", &mut portable)
        .expect_err("default migration ingest remains Portable")
        .to_string();
    assert!(error.contains("normalized_name"), "{error}");
    assert!(portable.tables.is_empty(), "failed create must not insert a partial table");

    let mut postgres = Schema::default();
    apply_postgres_migration(&mut postgres, &migration)
        .expect("explicit PostgreSQL migration ingest accepts the text cast");
    assert_eq!(
        column(table(&postgres, "people"), "normalized_name")
            .generated
            .as_ref()
            .unwrap()
            .expression,
        "first_name::text"
    );
}

/// Ensures survey mode skips a rejected migration operation, retains the prior schema, and records one gap.
#[test]
fn postgres_cast_migration_survey_keeps_the_last_valid_schema() {
    let mut schema = postgres_text_cast_schema();
    let original = schema.clone();
    survey::activate();
    let result = survey::unwrap_or_record(roundhouse::ingest::ingest_migration_with_generated_expression_dialect(
        br#"class ChangePeople < ActiveRecord::Migration[8.1]
  def change
    change_column :people, :first_name, :integer
  end
end
"#,
        "db/migrate/change_people.rb",
        &mut schema,
        GeneratedExpressionDialect::Postgres,
    ));
    let gaps = survey::drain();

    assert!(matches!(result, Ok(None)), "survey should skip the rejected operation: {result:?}");
    assert_eq!(schema, original, "survey recovery must retain valid table metadata");
    assert_eq!(gaps.len(), 1, "the rejected mutation should be ledgered once: {gaps:?}");
    assert!(gaps[0].to_string().contains("change_column"), "{gaps:?}");
}

#[test]
fn indexed_generated_outputs_cannot_be_removed_or_renamed() {
    for operation in [
        "remove_column :people, :display_name",
        "rename_column :people, :display_name, :label",
    ] {
        let mut schema = indexed_generated_schema();
        let original = schema.clone();
        let error = apply_migration(&mut schema, &one_operation(operation))
            .expect_err("changing an indexed generated output must fail explicitly");
        assert!(error.contains("generated column"), "{error}");
        assert!(error.contains("people.display_name"), "{error}");
        assert_eq!(
            schema, original,
            "rejected migration must retain its column and index"
        );
    }
}

#[test]
fn unindexed_generated_outputs_can_be_renamed_or_removed() {
    let mut renamed = generated_schema();
    apply_migration(
        &mut renamed,
        &one_operation("rename_column :people, :display_name, :label"),
    )
    .expect("an unindexed generated output can be renamed safely");
    let renamed_table = table(&renamed, "people");
    assert!(
        renamed_table
            .columns
            .iter()
            .all(|column| column.name.as_str() != "display_name")
    );
    let label = column(renamed_table, "label");
    let generated = label
        .generated
        .as_ref()
        .expect("renamed generated metadata");
    assert_eq!(generated.expression, "first_name || ' ' || last_name");
    assert_eq!(generated.storage, GeneratedColumnStorage::Stored);
    let ddl = render_schema_statements(&renamed);
    assert!(
        ddl[0].contains("label TEXT GENERATED ALWAYS AS (first_name || ' ' || last_name) STORED")
    );
    assert!(!ddl[0].contains("display_name"));

    let mut removed = generated_schema();
    apply_migration(
        &mut removed,
        &one_operation("remove_column :people, :display_name"),
    )
    .expect("an unindexed generated output can be removed safely");
    assert!(
        table(&removed, "people")
            .columns
            .iter()
            .all(|column| column.name.as_str() != "display_name")
    );
    let removed_ddl = render_schema_statements(&removed);
    assert!(!removed_ddl[0].contains("display_name"));
}

#[test]
fn unrelated_ordinary_column_changes_preserve_generated_metadata() {
    let mut schema = generated_schema();
    apply_migration(
        &mut schema,
        &one_operation(
            "rename_column :people, :nickname, :handle\n    remove_column :people, :obsolete\n    change_column :people, :first_name, :text, null: false",
        ),
    )
    .expect("ordinary column changes remain supported");

    let people = table(&schema, "people");
    assert!(
        people
            .columns
            .iter()
            .any(|column| column.name.as_str() == "handle")
    );
    assert!(
        people
            .columns
            .iter()
            .all(|column| column.name.as_str() != "nickname")
    );
    assert!(
        people
            .columns
            .iter()
            .all(|column| column.name.as_str() != "obsolete")
    );
    let first_name = column(people, "first_name");
    assert!(matches!(&first_name.col_type, ColumnType::Text));
    assert!(!first_name.nullable);

    let generated = column(people, "display_name")
        .generated
        .as_ref()
        .expect("unrelated ordinary changes must preserve generated metadata");
    assert_eq!(generated.expression, "first_name || ' ' || last_name");
    assert_eq!(generated.storage, GeneratedColumnStorage::Stored);
}
