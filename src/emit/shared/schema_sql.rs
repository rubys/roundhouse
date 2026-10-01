//! Target-neutral schema DDL rendering.
//!
//! Produces a single CREATE TABLE ... string covering every table in the
//! ingested `Schema`. Each per-target emitter embeds the result in its
//! generated project (e.g. a `pub const CREATE_TABLES: &str` in Rust,
//! a `CREATE_TABLES = """ ... """` in Python) so a fresh `:memory:`
//! SQLite connection can initialize via `execute_batch(CREATE_TABLES)`.
//!
//! Lives under `emit/shared/` rather than `lower/` because it produces
//! final target text, not a structured lower-level IR — the rest of
//! `lower/` is structured-to-structured in the compiler sense.
//!
//! The per-engine rendering sits behind [`Dialect`]; the `Schema` IR
//! itself stays dialect-neutral. Every caller renders
//! [`Dialect::Sqlite`] today. [`Dialect::Postgres`] is stage (a) of the
//! Postgres lane in roundhouse#91.

use std::fmt::Write;

use crate::schema::{Column, ColumnType, Schema};

/// The SQL engine a schema renders for: how it spells identifiers,
/// column types and key columns. The statements themselves are the same
/// in every dialect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    /// What every target ships today.
    #[default]
    Sqlite,
    /// Rails' PostgreSQL adapter's column types and default key
    /// conventions, applied to what ingest kept of the schema.
    Postgres,
}

impl Dialect {
    /// One physical identifier, preserving its spelling.
    ///
    /// SQLite keeps ordinary names bare (`naming::sql_ident`). Postgres
    /// quotes every identifier, as Rails' adapter does: an unquoted
    /// name there folds to lower case, so `createdAt` would no longer
    /// name the column Rails created, and its reserved words differ
    /// from SQLite's (`user` is one).
    pub fn ident(self, name: &str) -> String {
        match self {
            Dialect::Sqlite => crate::naming::sql_ident(name),
            Dialect::Postgres => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// A non-key column's type, as this dialect spells it.
    fn column_type(self, ct: &ColumnType) -> String {
        match self {
            Dialect::Sqlite => sqlite_type(ct).to_string(),
            Dialect::Postgres => postgres_type(ct),
        }
    }

    /// One primary-key column definition, name included. The IR keeps a
    /// key's name and type but not a `default:` given to `create_table`,
    /// so each dialect applies its default convention for the type.
    fn key_column(self, col: &Column) -> String {
        let name = self.ident(col.name.as_str());
        match self {
            // INTEGER PRIMARY KEY AUTOINCREMENT keeps rowids stable and
            // monotonic across inserts — matching what the Rails sqlite3
            // adapter emits for the default `id` column.
            Dialect::Sqlite if is_integer(&col.col_type) => {
                format!("  {name} INTEGER PRIMARY KEY AUTOINCREMENT")
            }
            // Rails' PostgreSQL adapter: `bigserial primary key` for the
            // default key, `serial` for `id: :integer`.
            Dialect::Postgres if is_integer(&col.col_type) => {
                let serial = if col.col_type == ColumnType::Integer { "serial" } else { "bigserial" };
                format!("  {name} {serial} PRIMARY KEY")
            }
            // `id: :uuid` gets Rails' default, `gen_random_uuid()` (core
            // since PostgreSQL 13). The ruby-shape insert still mints the
            // key itself, so the default only serves other writers.
            Dialect::Postgres if col.col_type == ColumnType::Uuid => {
                format!("  {name} uuid PRIMARY KEY NOT NULL DEFAULT gen_random_uuid()")
            }
            // `create_table …, id: :uuid` / `id: :string` on SQLite, or a
            // string key on Postgres: nothing generates the key, so the
            // VALUE is the app's to supply (#90).
            _ => format!("  {name} {} PRIMARY KEY NOT NULL", self.column_type(&col.col_type)),
        }
    }
}

/// Render every table + index in `schema` as a list of SQLite DDL
/// statements — one CREATE TABLE per table, one CREATE INDEX per
/// index. Idempotent (`IF NOT EXISTS`) so the runtime can re-run
/// against an existing DB without erroring.
///
/// Statements-list shape (rather than one joined string) is the
/// general form: portable across DB drivers that don't support
/// multi-statement execution (Postgres' pg gem, MySQL drivers),
/// and gives clearer per-statement error reporting in any adapter.
/// Adapters that DO accept multi-statement (better-sqlite3) just
/// `join("\n")` the list.
pub fn render_schema_statements(schema: &Schema) -> Vec<String> {
    render_schema_statements_for(schema, Dialect::Sqlite)
        .unwrap_or_else(|e| unreachable!("every schema renders as SQLite: {e}"))
}

/// [`render_schema_statements`] in `dialect`. The statements are the
/// same in every dialect — one CREATE TABLE per table, then one CREATE
/// INDEX per index, all `IF NOT EXISTS` — and only identifiers, column
/// types and key columns are spelled per engine.
///
/// `Err` names a table the dialect has no DDL for: a virtual table is
/// SQLite's own construct (Rails' SQLite3 adapter is the one that dumps
/// `create_virtual_table`), so SQLite never errors.
pub fn render_schema_statements_for(schema: &Schema, dialect: Dialect) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for (_name, table) in &schema.tables {
        // A virtual table is built by its MODULE, not from a column
        // list: no types, no NOT NULL, no AUTOINCREMENT key, and the
        // argument list is the module's own DSL rendered back verbatim
        // (fts5 mixes column names and `tokenize=…` options in one
        // list). Rendering it through the branch below produced DDL
        // sqlite rejects.
        if let Some(vm) = &table.virtual_module {
            if dialect != Dialect::Sqlite {
                return Err(format!(
                    "table `{}` is an SQLite virtual table (`USING {}`), which {dialect:?} has no DDL for",
                    table.name.as_str(),
                    vm.module
                ));
            }
            out.push(format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS {} USING {}({})",
                dialect.ident(table.name.as_str()),
                vm.module,
                vm.args.join(", ")
            ));
            continue;
        }
        let mut s = String::new();
        writeln!(s, "CREATE TABLE IF NOT EXISTS {} (", dialect.ident(table.name.as_str())).unwrap();
        let mut lines: Vec<String> = Vec::new();
        for col in &table.columns {
            if col.primary_key {
                lines.push(dialect.key_column(col));
                continue;
            }
            let mut line = format!(
                "  {} {}",
                dialect.ident(col.name.as_str()),
                dialect.column_type(&col.col_type)
            );
            if !col.nullable {
                line.push_str(" NOT NULL");
            }
            lines.push(line);
        }
        writeln!(s, "{}", lines.join(",\n")).unwrap();
        s.push(')');
        out.push(s);
    }
    for (_name, table) in &schema.tables {
        for idx in &table.indexes {
            let cols: Vec<String> = idx.columns.iter().map(|c| dialect.ident(c.as_str())).collect();
            let unique = if idx.unique { "UNIQUE " } else { "" };
            out.push(format!(
                "CREATE {unique}INDEX IF NOT EXISTS {} ON {} ({})",
                dialect.ident(idx.name.as_str()),
                dialect.ident(table.name.as_str()),
                cols.join(", "),
            ));
        }
    }
    Ok(out)
}

/// Joined-string form of `render_schema_statements` — kept for
/// per-target emitters that embed schema as a single `const` /
/// `let` declaration (Rust, Go, Python, Elixir, Crystal). Each
/// statement is `;`-terminated and joined with newlines so a single
/// `db.exec(joined)` call against a multi-statement-supporting
/// adapter executes them all.
pub fn render_schema_sql(schema: &Schema) -> String {
    let mut s = String::new();
    for stmt in render_schema_statements(schema) {
        s.push_str(&stmt);
        s.push_str(";\n");
    }
    s
}

fn is_integer(ct: &ColumnType) -> bool {
    matches!(ct, ColumnType::Integer | ColumnType::BigInt | ColumnType::Reference { .. })
}

/// Map a Roundhouse `ColumnType` to a SQLite storage class. SQLite's
/// type system is looser than most SQL engines; these mappings follow
/// what the Rails sqlite3 adapter emits so stored values round-trip
/// through both stacks.
fn sqlite_type(ct: &ColumnType) -> &'static str {
    match ct {
        ColumnType::Integer | ColumnType::BigInt => "INTEGER",
        ColumnType::Float | ColumnType::Decimal { .. } => "REAL",
        ColumnType::Boolean => "INTEGER",
        ColumnType::Binary => "BLOB",
        ColumnType::String { .. }
        | ColumnType::Text
        | ColumnType::Date
        | ColumnType::DateTime
        | ColumnType::Time
        | ColumnType::Json
        | ColumnType::Uuid => "TEXT",
        ColumnType::Reference { .. } => "INTEGER",
    }
}

/// Map a Roundhouse `ColumnType` to the type Rails' PostgreSQL adapter
/// creates for it (`NATIVE_DATABASE_TYPES`; `datetime` is Rails 7+'s
/// `timestamp(6)`), in the spelling `pg_dump` prints.
///
/// The IR is what ingest left: `jsonb` and `json` are both `Json`,
/// which renders `jsonb` — the type Postgres apps use, and the one that
/// has equality and a btree index; `timestamptz` is a `DateTime`, and
/// `citext` and the network types render as their text storage. A
/// decimal scale without a precision, which Rails rejects, renders as a
/// bare `numeric`.
fn postgres_type(ct: &ColumnType) -> String {
    match ct {
        ColumnType::Integer => "integer".into(),
        ColumnType::BigInt | ColumnType::Reference { .. } => "bigint".into(),
        ColumnType::Float => "double precision".into(),
        ColumnType::Decimal { precision: Some(p), scale: Some(s) } => format!("numeric({p},{s})"),
        ColumnType::Decimal { precision: Some(p), scale: None } => format!("numeric({p})"),
        ColumnType::Decimal { precision: None, .. } => "numeric".into(),
        ColumnType::String { limit: Some(n) } => format!("character varying({n})"),
        ColumnType::String { limit: None } => "character varying".into(),
        ColumnType::Text => "text".into(),
        ColumnType::Boolean => "boolean".into(),
        ColumnType::Date => "date".into(),
        ColumnType::DateTime => "timestamp(6) without time zone".into(),
        ColumnType::Time => "time without time zone".into(),
        ColumnType::Binary => "bytea".into(),
        ColumnType::Json => "jsonb".into(),
        ColumnType::Uuid => "uuid".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ident::{Symbol, TableRef};
    use crate::ingest::ingest_schema;
    use crate::schema::{Index, Table, VirtualModule};

    fn statements(schema_rb: &str, dialect: Dialect) -> Vec<String> {
        let schema = ingest_schema(schema_rb.as_bytes(), "db/schema.rb").expect("ingest schema");
        render_schema_statements_for(&schema, dialect).expect("render")
    }

    fn column(name: &str, col_type: ColumnType, nullable: bool, primary_key: bool) -> Column {
        Column { name: Symbol::from(name), col_type, nullable, default: None, primary_key }
    }

    fn table(name: &str, columns: Vec<Column>, indexes: Vec<Index>) -> Table {
        Table { name: Symbol::from(name), columns, indexes, foreign_keys: Vec::new(), virtual_module: None }
    }

    fn schema_of(tables: Vec<Table>) -> Schema {
        Schema { tables: tables.into_iter().map(|t| (t.name.clone(), t)).collect() }
    }

    const ARTICLES: &str = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "articles", force: :cascade do |t|
    t.string "title", null: false
    t.text "body"
    t.bigint "author_id", null: false
    t.boolean "published", default: false, null: false
    t.datetime "created_at", null: false
    t.index ["author_id"], name: "index_articles_on_author_id"
    t.index ["title", "author_id"], name: "index_articles_on_title_and_author_id", unique: true
  end
end
"#;

    /// The existing callers still call `render_schema_statements`,
    /// which renders the SQLite dialect, unchanged.
    #[test]
    fn sqlite_is_the_default_dialect() {
        assert_eq!(Dialect::default(), Dialect::Sqlite);
        let schema = ingest_schema(ARTICLES.as_bytes(), "db/schema.rb").unwrap();
        assert_eq!(Ok(render_schema_statements(&schema)), render_schema_statements_for(&schema, Dialect::Sqlite));
        assert_eq!(
            render_schema_statements(&schema),
            vec![
                "CREATE TABLE IF NOT EXISTS articles (\n  id INTEGER PRIMARY KEY AUTOINCREMENT,\n  \
                 title TEXT NOT NULL,\n  body TEXT,\n  author_id INTEGER NOT NULL,\n  \
                 published INTEGER NOT NULL,\n  created_at TEXT NOT NULL\n)",
                "CREATE INDEX IF NOT EXISTS index_articles_on_author_id ON articles (author_id)",
                "CREATE UNIQUE INDEX IF NOT EXISTS index_articles_on_title_and_author_id ON articles (title, author_id)",
            ]
        );
    }

    /// Column defaults are not rendered in either dialect (the model
    /// layer applies them), so `published` has no `DEFAULT false`.
    #[test]
    fn postgres_renders_rails_column_types_and_the_default_key() {
        assert_eq!(
            statements(ARTICLES, Dialect::Postgres),
            vec![
                "CREATE TABLE IF NOT EXISTS \"articles\" (\n  \"id\" bigserial PRIMARY KEY,\n  \
                 \"title\" character varying NOT NULL,\n  \"body\" text,\n  \
                 \"author_id\" bigint NOT NULL,\n  \"published\" boolean NOT NULL,\n  \
                 \"created_at\" timestamp(6) without time zone NOT NULL\n)",
                "CREATE INDEX IF NOT EXISTS \"index_articles_on_author_id\" ON \"articles\" (\"author_id\")",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_articles_on_title_and_author_id\" \
                 ON \"articles\" (\"title\", \"author_id\")",
            ]
        );
    }

    /// `create_table`'s key options get Rails' PostgreSQL defaults:
    /// `id: :integer` is a `serial`, `id: :uuid` defaults to
    /// `gen_random_uuid()`, a string key is the app's to supply, and
    /// `id: false` has no key at all.
    #[test]
    fn postgres_keys_follow_rails_defaults() {
        let out = statements(
            r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "counters", id: :integer, force: :cascade do |t|
    t.integer "value"
  end
  create_table "shares", id: :uuid, default: -> { "gen_random_uuid()" }, force: :cascade do |t|
    t.uuid "article_id", null: false
  end
  create_table "codes", primary_key: "code", id: :string, force: :cascade do |t|
    t.string "label", limit: 40
  end
  create_table "articles_tags", id: false, force: :cascade do |t|
    t.bigint "article_id", null: false
    t.bigint "tag_id", null: false
  end
end
"#,
            Dialect::Postgres,
        );
        assert_eq!(
            out,
            vec![
                "CREATE TABLE IF NOT EXISTS \"counters\" (\n  \"id\" serial PRIMARY KEY,\n  \"value\" integer\n)",
                "CREATE TABLE IF NOT EXISTS \"shares\" (\n  \
                 \"id\" uuid PRIMARY KEY NOT NULL DEFAULT gen_random_uuid(),\n  \
                 \"article_id\" uuid NOT NULL\n)",
                "CREATE TABLE IF NOT EXISTS \"codes\" (\n  \"code\" character varying PRIMARY KEY NOT NULL,\n  \
                 \"label\" character varying(40)\n)",
                "CREATE TABLE IF NOT EXISTS \"articles_tags\" (\n  \"article_id\" bigint NOT NULL,\n  \
                 \"tag_id\" bigint NOT NULL\n)",
            ]
        );
    }

    #[test]
    fn postgres_types_follow_rails_postgresql_adapter() {
        let cases = [
            (ColumnType::Integer, "integer"),
            (ColumnType::BigInt, "bigint"),
            (ColumnType::Reference { table: TableRef(Symbol::from("authors")) }, "bigint"),
            (ColumnType::Float, "double precision"),
            (ColumnType::Decimal { precision: None, scale: None }, "numeric"),
            (ColumnType::Decimal { precision: Some(10), scale: None }, "numeric(10)"),
            (ColumnType::Decimal { precision: Some(10), scale: Some(2) }, "numeric(10,2)"),
            (ColumnType::String { limit: None }, "character varying"),
            (ColumnType::String { limit: Some(40) }, "character varying(40)"),
            (ColumnType::Text, "text"),
            (ColumnType::Boolean, "boolean"),
            (ColumnType::Date, "date"),
            (ColumnType::DateTime, "timestamp(6) without time zone"),
            (ColumnType::Time, "time without time zone"),
            (ColumnType::Binary, "bytea"),
            // Ingest folds `jsonb` and `json` into one `Json`.
            (ColumnType::Json, "jsonb"),
            (ColumnType::Uuid, "uuid"),
            // Not something Rails emits (it rejects a scale without a
            // precision): the scale is dropped rather than a precision
            // invented.
            (ColumnType::Decimal { precision: None, scale: Some(2) }, "numeric"),
        ];
        for (ct, want) in cases {
            assert_eq!(Dialect::Postgres.column_type(&ct), want, "{ct:?}");
        }
    }

    /// `user` is reserved in Postgres but not in SQLite, an unquoted
    /// `createdAt` would fold to `createdat` there, and an embedded quote
    /// is doubled in both — for table, key, column and index names alike.
    #[test]
    fn postgres_quotes_every_identifier() {
        let schema = schema_of(vec![table(
            "user",
            vec![
                column("ID", ColumnType::BigInt, false, true),
                column("createdAt", ColumnType::DateTime, false, false),
                column("odd\"name", ColumnType::Decimal { precision: Some(10), scale: Some(2) }, true, false),
            ],
            vec![Index { name: Symbol::from("index_user_on_createdAt"), columns: vec![Symbol::from("createdAt")], unique: true }],
        )]);
        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Postgres).unwrap(),
            vec![
                "CREATE TABLE IF NOT EXISTS \"user\" (\n  \"ID\" bigserial PRIMARY KEY,\n  \
                 \"createdAt\" timestamp(6) without time zone NOT NULL,\n  \"odd\"\"name\" numeric(10,2)\n)",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_user_on_createdAt\" ON \"user\" (\"createdAt\")",
            ]
        );
        assert_eq!(
            render_schema_statements(&schema),
            vec![
                "CREATE TABLE IF NOT EXISTS user (\n  ID INTEGER PRIMARY KEY AUTOINCREMENT,\n  \
                 createdAt TEXT NOT NULL,\n  \"odd\"\"name\" REAL\n)",
                "CREATE UNIQUE INDEX IF NOT EXISTS index_user_on_createdAt ON user (createdAt)",
            ]
        );
    }

    /// A virtual table is SQLite's own construct: SQLite renders it, and
    /// Postgres refuses the schema rather than returning DDL it rejects.
    #[test]
    fn a_virtual_table_has_no_postgres_ddl() {
        let mut search = table("message_search_index", Vec::new(), Vec::new());
        search.virtual_module =
            Some(VirtualModule { module: "fts5".into(), args: vec!["body".into(), "tokenize=porter".into()] });
        let schema = schema_of(vec![search]);
        assert_eq!(
            render_schema_statements(&schema),
            vec!["CREATE VIRTUAL TABLE IF NOT EXISTS message_search_index USING fts5(body, tokenize=porter)"]
        );
        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Postgres),
            Err("table `message_search_index` is an SQLite virtual table (`USING fts5`), \
                 which Postgres has no DDL for"
                .to_string())
        );
    }
}
