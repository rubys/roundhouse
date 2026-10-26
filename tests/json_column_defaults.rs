use roundhouse::emit::shared::schema_sql::{Dialect, render_schema_statements_for};
use roundhouse::ingest::structure_sql::ingest_structure_sql;
use roundhouse::ingest::{ingest_migration, ingest_schema};
use roundhouse::schema::Schema;

#[path = "support/emit_and_run.rs"]
mod emit_and_run;

const DEFAULTS: &[(&str, &str, &str)] = &[
    ("empty_array", "[]", "[]"),
    ("empty_object", "{}", "{}"),
    (
        "items",
        r#"[1, "two", true, false, nil, -3, -1.25]"#,
        r#"[1,"two",true,false,null,-3,-1.25]"#,
    ),
    (
        "nested",
        r#"{ "z" => [1, { "a" => "O'Reilly\\docs\"" }], enabled: true }"#,
        r#"{"z":[1,{"a":"O'Reilly\\docs\""}],"enabled":true}"#,
    ),
    ("text", r#""[]""#, r#""[]""#),
    ("number", "42", "42"),
    ("boolean", "false", "false"),
    (
        "keys",
        r#"{ :a => 1, "a" => 2, 3 => :three, nil => false, "b" => 1, "b" => 2 }"#,
        r#"{"a":1,"a":2,"3":"three","":false,"b":2}"#,
    ),
];

fn schema_source() -> String {
    let mut source =
        String::from("ActiveRecord::Schema[8.1].define do\n  create_table :documents do |t|\n");
    for kind in ["json", "jsonb"] {
        for (name, literal, _) in DEFAULTS {
            source.push_str(&format!(
                "    t.{kind} :{kind}_{name}, default: {literal}, null: false\n"
            ));
        }
        source.push_str(&format!("    t.{kind} :{kind}_null, default: nil\n"));
        source.push_str(&format!("    t.{kind} :{kind}_unset\n"));
        source.push_str(&format!(
            "    t.{kind} :{kind}_expression, default: -> {{ \"'[]'\" }}\n"
        ));
        source.push_str(&format!(
            "    t.{kind} :{kind}_dynamic, default: [ENV.fetch(\"DEFAULT_ITEM\")]\n"
        ));
    }
    source.push_str("    t.string :ordinary, default: \"[]\"\n  end\nend\n");
    source
}

fn assert_defaults(schema: &Schema) {
    let table = schema
        .tables
        .values()
        .find(|t| t.name.as_str() == "documents")
        .unwrap();
    for kind in ["json", "jsonb"] {
        for (name, _, expected) in DEFAULTS {
            let name = format!("{kind}_{name}");
            let column = table
                .columns
                .iter()
                .find(|c| c.name.as_str() == name)
                .unwrap();
            assert_eq!(column.default.as_deref(), Some(*expected), "{name}");
        }
        for suffix in ["null", "unset", "expression", "dynamic"] {
            let name = format!("{kind}_{suffix}");
            let column = table
                .columns
                .iter()
                .find(|c| c.name.as_str() == name)
                .unwrap();
            assert_eq!(column.default, None, "{name}");
        }
    }
}

#[test]
fn schema_rb_keeps_serialized_json_defaults() {
    let schema = ingest_schema(schema_source().as_bytes(), "db/schema.rb").unwrap();
    assert_defaults(&schema);
    let round_trip: Schema =
        serde_json::from_str(&serde_json::to_string(&schema).unwrap()).unwrap();
    assert_defaults(&round_trip);
}

#[test]
fn sqlite_and_postgres_ddl_quote_json_defaults() {
    let schema = ingest_schema(schema_source().as_bytes(), "db/schema.rb").unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect)
            .unwrap()
            .join(";\n");
        for kind in ["json", "jsonb"] {
            for (name, _, expected) in DEFAULTS {
                let column = dialect.ident(&format!("{kind}_{name}"));
                let storage = if dialect == Dialect::Sqlite {
                    "TEXT"
                } else {
                    kind
                };
                let expected = expected.replace('\'', "''");
                let line = format!("{column} {storage} NOT NULL DEFAULT '{expected}'");
                assert!(ddl.contains(&line), "missing {line}\n{ddl}");
            }
        }
        assert!(
            ddl.contains(&format!(
                "{} {} DEFAULT '[]'",
                dialect.ident("ordinary"),
                if dialect == Dialect::Sqlite {
                    "TEXT"
                } else {
                    "character varying"
                }
            )),
            "{ddl}"
        );
    }
}

#[test]
fn migration_create_add_and_change_column_keep_json_defaults() {
    let mut schema = Schema::default();
    let migration = schema_source().replacen(
        "ActiveRecord::Schema[8.1].define do",
        "class CreateDocuments < ActiveRecord::Migration[8.1]\n  def change",
        1,
    ) + "end\n";
    ingest_migration(
        migration.as_bytes(),
        "db/migrate/create_documents.rb",
        &mut schema,
    )
    .unwrap();
    assert_defaults(&schema);
    ingest_migration(
        br#"class ChangeDocuments < ActiveRecord::Migration[8.1]
  def change
    add_column :documents, :added, :json, default: { "a" => [] }
    change_column :documents, :added, :jsonb, default: [1, { "b" => false }]
  end
end"#,
        "db/migrate/change_documents.rb",
        &mut schema,
    )
    .unwrap();
    let column = schema
        .tables
        .values()
        .next()
        .unwrap()
        .columns
        .iter()
        .find(|c| c.name.as_str() == "added")
        .unwrap();
    assert_eq!(column.default.as_deref(), Some(r#"[1,{"b":false}]"#));
}

#[test]
fn change_column_default_serializes_json_literals_and_to_values() {
    let mut schema = ingest_schema(schema_source().as_bytes(), "db/schema.rb").unwrap();
    for (statement, expected) in [
        (
            r#"change_column_default :documents, :json_empty_array, { "a" => [] }"#,
            Some(r#"{"a":[]}"#),
        ),
        (
            r#"change_column_default :documents, :json_empty_array, from: {}, to: [1, false]"#,
            Some("[1,false]"),
        ),
        (
            r#"change_column_default :documents, :json_empty_array, "[]""#,
            Some(r#""[]""#),
        ),
        (
            "change_column_default :documents, :json_empty_array, from: [], to: nil",
            None,
        ),
    ] {
        let migration = format!(
            "class ChangeDocuments < ActiveRecord::Migration[8.1]\n  def change\n    {statement}\n  end\nend"
        );
        ingest_migration(migration.as_bytes(), "db/migrate/default.rb", &mut schema).unwrap();
        let column = schema
            .tables
            .values()
            .next()
            .unwrap()
            .columns
            .iter()
            .find(|c| c.name.as_str() == "json_empty_array")
            .unwrap();
        assert_eq!(column.default.as_deref(), expected, "{statement}");
    }
}

#[test]
fn structure_sql_json_defaults_are_already_serialized() {
    let schema = ingest_structure_sql(
        br#"CREATE TABLE public.documents (
  id bigint NOT NULL,
  payload json DEFAULT '{"a": [1, true]}'::json NOT NULL,
  text_payload jsonb DEFAULT '"[]"'::jsonb NOT NULL
);
ALTER TABLE public.documents ALTER COLUMN payload SET DEFAULT '{"b": []}'::json;"#,
        "db/structure.sql",
    )
    .unwrap();
    for dialect in [Dialect::Sqlite, Dialect::Postgres] {
        let ddl = render_schema_statements_for(&schema, dialect)
            .unwrap()
            .join(";\n");
        assert!(ddl.contains(r#"DEFAULT '{"b": []}'"#), "{ddl}");
        assert!(ddl.contains(r#"DEFAULT '"[]"'"#), "{ddl}");
    }
}

#[test]
fn emitted_raw_insert_uses_json_defaults() {
    let run = emit_and_run::real_blog()
        .edit("db/schema.rb", "create_table \"articles\", force: :cascade do |t|", "create_table \"articles\", force: :cascade do |t|\n    t.json :paths, default: [], null: false\n    t.jsonb :settings, default: { \"enabled\" => true, \"items\" => [1, nil] }, null: false")
        .write("test/models/json_defaults_test.rb", r#"require "test_helper"
class JsonDefaultsTest < ActiveSupport::TestCase
  test "raw inserts receive serialized database defaults" do
    ActiveRecord::Base.connection.execute("INSERT INTO articles (title, body, created_at, updated_at) VALUES ('defaults', 'A body long enough for validation.', '2026-01-01', '2026-01-01')")
    article = Article.where(title: "defaults").first
    assert_equal [], article.paths
    assert_equal({ "enabled" => true, "items" => [1, nil] }, article.settings)
  end
end
"#)
        .run_test("test/models/json_defaults_test.rb");
    run.assert_passes();
}
