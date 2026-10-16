//! Column-option forms that cannot be inspected must fail closed, so a
//! generated column cannot silently enter the schema as an ordinary column.

use roundhouse::ingest::{ingest_migration, ingest_schema};
use roundhouse::schema::{ColumnType, Schema};

fn schema_error(source: &str) -> String {
    ingest_schema(source.as_bytes(), "db/schema.rb")
        .expect_err("unresolved generated-column options must be rejected")
        .to_string()
}

fn migration_error(source: &str) -> String {
    let mut schema = Schema::default();
    ingest_migration(
        source.as_bytes(),
        "db/migrate/add_generated_label.rb",
        &mut schema,
    )
    .expect_err("unresolved generated-column options must be rejected")
    .to_string()
}

#[test]
fn schema_column_option_splats_do_not_become_ordinary_columns() {
    for splat in [
        "**options",
        r#"**{ as: "first_name || ' ' || last_name", stored: true }"#,
    ] {
        let source = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.string "display_name", {splat}
  end
end
"#
        );
        let error = schema_error(&source);
        assert!(error.contains("db/schema.rb"), "{error}");
        assert!(error.contains("display_name"), "{error}");
        assert!(error.contains("keyword splats"), "{error}");
    }
}

#[test]
fn migration_column_option_splats_do_not_become_ordinary_columns() {
    for splat in [
        "**options",
        r#"**{ as: "first_name || ' ' || last_name", stored: true }"#,
    ] {
        let source = format!(
            r#"class AddDisplayName < ActiveRecord::Migration[8.1]
  def change
    add_column :people, :display_name, :string, {splat}
  end
end
"#
        );
        let error = migration_error(&source);
        assert!(
            error.contains("db/migrate/add_generated_label.rb"),
            "{error}"
        );
        assert!(error.contains("people.display_name"), "{error}");
        assert!(error.contains("keyword splats"), "{error}");
    }
}

#[test]
fn braced_generated_options_are_not_ignored_as_positional_hashes() {
    let schema_source = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "display_name", { as: "first_name", stored: true }
  end
end
"#;
    let error = schema_error(schema_source);
    assert!(error.contains("db/schema.rb"), "{error}");
    assert!(error.contains("generated column options"), "{error}");

    let migration_source = r#"class AddDisplayName < ActiveRecord::Migration[8.1]
  def change
    add_column :people, :display_name, :string, { as: "first_name", stored: true }
  end
end
"#;
    let error = migration_error(migration_source);
    assert!(
        error.contains("db/migrate/add_generated_label.rb"),
        "{error}"
    );
    assert!(error.contains("generated column options"), "{error}");
}

#[test]
fn references_type_options_remain_ordinary_schema_and_migration_columns() {
    for association in ["references", "belongs_to"] {
        for options in ["type: :uuid", "{ type: :uuid }"] {
            let schema_source = format!(
                r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "events", force: :cascade do |t|
    t.{association} "account", {options}
  end
end
"#
            );
            let schema = ingest_schema(schema_source.as_bytes(), "db/schema.rb")
                .expect("ordinary reference type options must not look generated");
            let column = schema.tables[&roundhouse::Symbol::from("events")]
                .columns
                .iter()
                .find(|column| column.name.as_str() == "account")
                .expect("reference column");
            assert!(
                matches!(&column.col_type, ColumnType::Reference { .. }),
                "schema.rb `t.{association}` should keep the existing Reference type, got {:?}",
                column.col_type
            );
            assert!(column.generated.is_none());

            let migration_source = format!(
                r#"class CreateEvents < ActiveRecord::Migration[8.1]
  def change
    create_table :events do |t|
      t.{association} :account, {options}
    end
  end
end
"#
            );
            let mut migrated = Schema::default();
            ingest_migration(
                migration_source.as_bytes(),
                "db/migrate/create_events.rb",
                &mut migrated,
            )
            .expect("ordinary migration reference type options must not look generated");
            let column = migrated.tables[&roundhouse::Symbol::from("events")]
                .columns
                .iter()
                .find(|column| column.name.as_str() == "account")
                .expect("migration reference column");
            assert!(
                matches!(&column.col_type, ColumnType::Reference { .. }),
                "migration `t.{association}` should keep the existing Reference type, got {:?}",
                column.col_type
            );
            assert!(column.generated.is_none());
        }
    }
}

#[test]
fn ordinary_change_column_still_updates_an_ordinary_schema_column() {
    let mut schema = ingest_schema(
        br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
  end
end
"#,
        "db/schema.rb",
    )
    .expect("ordinary schema column");
    ingest_migration(
        br#"class ChangeFirstName < ActiveRecord::Migration[8.1]
  def change
    change_column :people, :first_name, :text, null: false
  end
end
"#,
        "db/migrate/change_first_name.rb",
        &mut schema,
    )
    .expect("ordinary change_column remains supported");

    let column = schema.tables[&roundhouse::Symbol::from("people")]
        .columns
        .iter()
        .find(|column| column.name.as_str() == "first_name")
        .expect("changed column");
    assert!(matches!(&column.col_type, ColumnType::Text));
    assert!(!column.nullable);
    assert!(column.generated.is_none());
}

#[test]
fn migration_replacements_do_not_drop_existing_generated_metadata() {
    for operation in [
        "change_column :people, :display_name, :string, null: false",
        "change_column :people, :display_name, :unknown_type",
        "add_column :people, :display_name, :string, null: false",
    ] {
        let mut schema = ingest_schema(
            br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "last_name"
    t.virtual "display_name", type: :string, as: "first_name || ' ' || last_name", stored: true
  end
end
"#,
            "db/schema.rb",
        )
        .expect("generated schema column");
        let migration = format!(
            r#"class ReplaceDisplayName < ActiveRecord::Migration[8.1]
  def change
    {operation}
  end
end
"#
        );
        let error = ingest_migration(
            migration.as_bytes(),
            "db/migrate/replace_display_name.rb",
            &mut schema,
        )
        .expect_err("ordinary replacement must not erase generated metadata")
        .to_string();
        assert!(error.contains("people.display_name"), "{error}");
        assert!(error.contains("generated column"), "{error}");

        let column = schema.tables[&roundhouse::Symbol::from("people")]
            .columns
            .iter()
            .find(|column| column.name.as_str() == "display_name")
            .expect("rejected migration must leave the existing column in place");
        let generated = column
            .generated
            .as_ref()
            .expect("rejected migration must retain generated metadata");
        assert_eq!(
            generated.expression, "first_name || ' ' || last_name",
            "{operation}"
        );
    }
}

#[test]
fn keyword_as_and_stored_options_still_fail_on_ordinary_column_calls() {
    for options in [
        r#"as: "first_name""#,
        r#"{ as: "first_name" }"#,
        "stored: true",
        "{ stored: true }",
        r#"as: "first_name", stored: true"#,
        r#"{ as: "first_name", stored: true }"#,
    ] {
        let schema_source = format!(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "people", force: :cascade do |t|
    t.string "first_name"
    t.string "display_name", {options}
  end
end
"#
        );
        let error = schema_error(&schema_source);
        assert!(error.contains("people"), "{error}");
        assert!(error.contains("generated column options"), "{error}");

        let migration_source = format!(
            r#"class CreatePeople < ActiveRecord::Migration[8.1]
  def change
    create_table :people do |t|
      t.string :first_name
      t.string :display_name, {options}
    end
  end
end
"#
        );
        let error = migration_error(&migration_source);
        assert!(error.contains("people"), "{error}");
        assert!(error.contains("generated column options"), "{error}");
    }
}
