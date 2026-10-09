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

use crate::schema::{Column, ColumnType, GeneratedColumnStorage, Index, Schema, Table};

/// The SQL engine a schema renders for: how it spells identifiers,
/// column types and key columns, and which partial-index predicates it
/// keeps. The statements themselves are the same in every dialect.
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

    /// The `WHERE` of a partial index, when this dialect renders it.
    /// The predicate is the source database's SQL, rendered as written.
    ///
    /// Postgres renders every one. SQLite renders a unique index's,
    /// because there it decides which rows must be distinct: without it
    /// the index rejects rows Rails accepts. It leaves out two kinds:
    ///
    /// - A non-unique index's. It only narrows which rows the index
    ///   covers, and a Postgres dump often spells it in syntax SQLite
    ///   cannot parse (Mastodon's `(state = ANY (ARRAY[2, 3]))`).
    /// - One SQLite can't be trusted to run as written (see
    ///   [`sqlite_reads_alike`]). That unique index covers every row, as
    ///   it did before predicates were kept, and
    ///   `project::report_sqlite_index_predicates` names it.
    ///
    /// The `insert_all` guard and the upsert conflict target read the
    /// same answer, so all three agree on which rows are constrained.
    pub fn index_predicate<'a>(self, table: &Table, idx: &'a Index) -> Option<&'a str> {
        let predicate = idx.predicate.as_deref()?;
        match self {
            Dialect::Sqlite if !idx.unique || !sqlite_reads_alike(predicate, table) => None,
            _ => Some(predicate),
        }
    }

    /// One primary-key column definition, name included. The IR keeps a
    /// key's name and type but not a `default:` given to `create_table`,
    /// so each dialect applies its default convention for the type.
    /// The column's `default:` as a SQL literal, for the writes that
    /// never pass through a model — `insert_all`, `upsert_all`, a raw
    /// `INSERT` — which in Rails get the DATABASE's default. (A record
    /// gets the same value from the model layer either way.) Only the
    /// unambiguous literal kinds render: a number on a numeric column, a
    /// boolean, a string on a string column. Anything else — a date, a
    /// JSON or binary value, a default ingest could not read — is left to
    /// the model layer, as before.
    fn column_default(self, col: &Column) -> Option<String> {
        let raw = col.default.as_deref()?;
        if col.generated.is_some() {
            return None;
        }
        match &col.col_type {
            ColumnType::Integer | ColumnType::BigInt => {
                raw.parse::<i64>().ok().map(|n| n.to_string())
            }
            ColumnType::Float | ColumnType::Decimal { .. } => {
                raw.parse::<f64>().ok().filter(|f| f.is_finite()).map(|_| raw.to_string())
            }
            ColumnType::Boolean => match (raw, self) {
                ("true", Dialect::Sqlite) => Some("1".to_string()),
                ("false", Dialect::Sqlite) => Some("0".to_string()),
                ("true", Dialect::Postgres) => Some("TRUE".to_string()),
                ("false", Dialect::Postgres) => Some("FALSE".to_string()),
                _ => None,
            },
            ColumnType::String { .. } | ColumnType::Text => {
                Some(format!("'{}'", raw.replace('\'', "''")))
            }
            _ => None,
        }
    }

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
/// `join("\n")` the list. The IR-facing API is infallible for its
/// existing callers; an invalid generated-column schema reports an
/// Unsupported diagnostic and returns no partial DDL.
pub fn render_schema_statements(schema: &Schema) -> Vec<String> {
    match render_schema_statements_for(schema, Dialect::Sqlite) {
        Ok(statements) => statements,
        Err(error) => {
            let _ = crate::emit::diagnostics::report_unsupported(
                crate::span::Span::synthetic(),
                "sqlite",
                "generated schema",
                error,
            );
            Vec::new()
        }
    }
}

/// [`render_schema_statements`] in `dialect`. The statements are the
/// same in every dialect — one CREATE TABLE per table, then one CREATE
/// INDEX per index, all `IF NOT EXISTS` — and only identifiers, column
/// types, key columns and partial-index predicates
/// ([`Dialect::index_predicate`]) differ per engine.
///
/// `Err` names a schema shape this dialect cannot represent, including
/// unsupported generated expressions and virtual tables on PostgreSQL.
pub fn render_schema_statements_for(schema: &Schema, dialect: Dialect) -> Result<Vec<String>, String> {
    validate_schema_for_dialect(schema, dialect)?;
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
            if let Some(generated) = &col.generated {
                line.push_str(" GENERATED ALWAYS AS (");
                line.push_str(&generated.expression);
                line.push_str(") ");
                line.push_str(match generated.storage {
                    GeneratedColumnStorage::Stored => "STORED",
                    GeneratedColumnStorage::Virtual => "VIRTUAL",
                });
            }
            if !col.nullable {
                line.push_str(" NOT NULL");
            }
            if let Some(default) = dialect.column_default(col) {
                line.push_str(" DEFAULT ");
                line.push_str(&default);
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
            let using = match (dialect, idx.using.as_deref()) {
                (Dialect::Postgres, Some(method)) => format!(" USING {}", dialect.ident(method)),
                _ => String::new(),
            };
            let predicate = dialect
                .index_predicate(table, idx)
                .map(|p| format!(" WHERE {p}"))
                .unwrap_or_default();
            out.push(format!(
                "CREATE {unique}INDEX IF NOT EXISTS {} ON {}{using} ({}){predicate}",
                dialect.ident(idx.name.as_str()),
                dialect.ident(table.name.as_str()),
                cols.join(", "),
            ));
        }
    }
    Ok(out)
}

/// Reject a schema that a target dialect cannot represent. Current
/// SQLite target callers use the infallible wrapper, so project assembly
/// invokes this gate before emitting any files; the PostgreSQL renderer
/// also checks it directly.
pub fn validate_schema_for_dialect(schema: &Schema, dialect: Dialect) -> Result<(), String> {
    let expression_dialect = match dialect {
        Dialect::Sqlite => crate::schema::generated::GeneratedExpressionDialect::Portable,
        Dialect::Postgres => crate::schema::generated::GeneratedExpressionDialect::Postgres,
    };
    for table in schema.tables.values() {
        if let Some(vm) = &table.virtual_module {
            if dialect != Dialect::Sqlite {
                return Err(format!(
                    "table `{}` is an SQLite virtual table (`USING {}`), which {dialect:?} has no DDL for",
                    table.name.as_str(),
                    vm.module
                ));
            }
        }
        for (column, reason) in
            crate::schema::generated::validate_table_with_dialect(table, expression_dialect)
        {
            return Err(format!("generated column unsupported: {}.{column}: {reason}", table.name.as_str()));
        }
        if dialect == Dialect::Postgres {
            if let Some(column) = table.columns.iter().find(|column| {
                column.generated.as_ref().is_some_and(|generated| {
                    generated.storage == GeneratedColumnStorage::Virtual
                })
                }) {
                return Err(format!(
                    "Roundhouse PostgreSQL DDL renderer does not yet support virtual generated columns: `{}.{}`",
                    table.name.as_str(),
                    column.name.as_str()
                ));
            }
        }
    }
    Ok(())
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

/// Whether SQLite reads `predicate`, a partial index's `WHERE` as the
/// source database wrote it, the way that database does. An allow-list,
/// because a Postgres dump spells much that SQLite either can't parse
/// (`(kind)::text`, `= ANY (ARRAY[2, 3])`, `~~`, `@>`) or parses as
/// something else (`enabled IS UNKNOWN`, where `UNKNOWN` becomes a column
/// name). Outside its string literals the predicate may use only:
///
/// - the table's own columns, bare or double-quoted;
/// - numbers;
/// - `AND`, `OR`, `NOT`, `IS`, `NULL`, `TRUE`, `FALSE`, `IN` and
///   `BETWEEN` (not `LIKE`, which ignores ASCII case in SQLite and not in
///   Postgres);
/// - the functions both engines define alike (on ASCII text), called as
///   `lower(…)`: `lower`, `upper`, `length`, `coalesce`, `nullif` and
///   `abs`;
/// - parentheses, commas, and the operators `= <> != < > <= >= + - * / %
///   ||`.
///
/// No rewriting: dropping a `::text` cast is not generally
/// meaning-preserving (`status::text = '01'` is false for an integer 1
/// in Postgres, and `status = '01'` is true in SQLite).
fn sqlite_reads_alike(predicate: &str, table: &Table) -> bool {
    const WORDS: &[&str] = &["AND", "OR", "NOT", "IS", "NULL", "TRUE", "FALSE", "IN", "BETWEEN"];
    const FUNCTIONS: &[&str] = &["lower", "upper", "length", "coalesce", "nullif", "abs"];
    let is_column = |name: &str, quoted: bool| {
        table.columns.iter().any(|c| {
            if quoted {
                c.name.as_str() == name
            } else {
                c.name.as_str().eq_ignore_ascii_case(name)
            }
        })
    };
    let chars: Vec<char> = predicate.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // A string literal; `''` inside it is an escaped quote.
            i += 1;
            loop {
                match chars.get(i) {
                    None => return false,
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => i += 2,
                    Some('\'') => break,
                    Some(_) => i += 1,
                }
            }
            i += 1;
        } else if c == '"' {
            let mut name = String::new();
            i += 1;
            loop {
                match chars.get(i) {
                    None => return false,
                    Some('"') if chars.get(i + 1) == Some(&'"') => {
                        name.push('"');
                        i += 2;
                    }
                    Some('"') => break,
                    Some(&ch) => {
                        name.push(ch);
                        i += 1;
                    }
                }
            }
            i += 1;
            if !is_column(&name, true) {
                return false;
            }
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while chars.get(i).is_some_and(|ch| ch.is_ascii_alphanumeric() || *ch == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let mut next = i;
            while chars.get(next).is_some_and(|ch| ch.is_ascii_whitespace()) {
                next += 1;
            }
            let ok = if WORDS.iter().any(|w| word.eq_ignore_ascii_case(w)) {
                true
            } else if chars.get(next) == Some(&'(') {
                FUNCTIONS.iter().any(|f| word.eq_ignore_ascii_case(f))
            } else {
                is_column(&word, false)
            };
            if !ok {
                return false;
            }
        } else if c.is_ascii_digit() {
            while chars.get(i).is_some_and(|ch| ch.is_ascii_digit() || *ch == '.') {
                i += 1;
            }
            if chars.get(i).is_some_and(|ch| ch.is_ascii_alphabetic() || *ch == '_') {
                return false;
            }
        } else if c.is_ascii_whitespace()
            || matches!(c, '(' | ')' | ',' | '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '%' | '|')
        {
            i += 1;
        } else {
            return false;
        }
    }
    true
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
        | ColumnType::Jsonb
        | ColumnType::Uuid => "TEXT",
        ColumnType::Reference { .. } => "INTEGER",
    }
}

/// Map a Roundhouse `ColumnType` to the type Rails' PostgreSQL adapter
/// creates for it (`NATIVE_DATABASE_TYPES`; `datetime` is Rails 7+'s
/// `timestamp(6)`), in the spelling `pg_dump` prints.
///
/// Ingest preserves `json` and `jsonb` independently so PostgreSQL schema
/// output retains the source type. `timestamptz` is a `DateTime`, and
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
        ColumnType::Json => "json".into(),
        ColumnType::Jsonb => "jsonb".into(),
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

    /// Builds a plain schema column without generated expressions or source-type provenance.
    fn column(name: &str, col_type: ColumnType, nullable: bool, primary_key: bool) -> Column {
        Column {
            name: Symbol::from(name),
            col_type,
            nullable,
            default: None,
            primary_key,
            generated: None,
            generated_text_compatible: None,
            generated_int4_compatible: None,
        }
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
                 published INTEGER NOT NULL DEFAULT 0,\n  created_at TEXT NOT NULL\n)",
                "CREATE INDEX IF NOT EXISTS index_articles_on_author_id ON articles (author_id)",
                "CREATE UNIQUE INDEX IF NOT EXISTS index_articles_on_title_and_author_id ON articles (title, author_id)",
            ]
        );
    }

    /// A literal column default renders in each dialect's spelling —
    /// `published`'s `default: false` is `DEFAULT FALSE` here and
    /// `DEFAULT 0` on SQLite — for the writes that bypass the model
    /// layer (`insert_all`).
    #[test]
    fn postgres_renders_rails_column_types_and_the_default_key() {
        assert_eq!(
            statements(ARTICLES, Dialect::Postgres),
            vec![
                "CREATE TABLE IF NOT EXISTS \"articles\" (\n  \"id\" bigserial PRIMARY KEY,\n  \
                 \"title\" character varying NOT NULL,\n  \"body\" text,\n  \
                 \"author_id\" bigint NOT NULL,\n  \"published\" boolean NOT NULL DEFAULT FALSE,\n  \
                 \"created_at\" timestamp(6) without time zone NOT NULL\n)",
                "CREATE INDEX IF NOT EXISTS \"index_articles_on_author_id\" ON \"articles\" (\"author_id\")",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_articles_on_title_and_author_id\" \
                 ON \"articles\" (\"title\", \"author_id\")",
            ]
        );
    }

    #[test]
    fn postgres_preserves_btree_gin_and_custom_index_methods() {
        let schema_rb = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.jsonb "payload"
    t.string "slug"
    t.index ["payload"], name: "index_documents_on_payload_gin", using: :gin
    t.index ["slug"], name: "index_documents_on_slug_btree", using: :btree
    t.index ["slug"], name: "index_documents_on_slug_custom", using: "custom_method"
  end
end
"#;
        let schema = ingest_schema(schema_rb.as_bytes(), "db/schema.rb").expect("schema");
        let indexes = &schema.tables[&Symbol::from("documents")].indexes;
        assert_eq!(indexes[0].using.as_deref(), Some("gin"));
        assert_eq!(indexes[1].using.as_deref(), Some("btree"));
        assert_eq!(indexes[2].using.as_deref(), Some("custom_method"));

        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Postgres).unwrap()[1..],
            [
                "CREATE INDEX IF NOT EXISTS \"index_documents_on_payload_gin\" ON \"documents\" USING \"gin\" (\"payload\")",
                "CREATE INDEX IF NOT EXISTS \"index_documents_on_slug_btree\" ON \"documents\" USING \"btree\" (\"slug\")",
                "CREATE INDEX IF NOT EXISTS \"index_documents_on_slug_custom\" ON \"documents\" USING \"custom_method\" (\"slug\")",
            ]
        );
        // PostgreSQL method metadata is intentionally absent from SQLite DDL.
        assert_eq!(
            render_schema_statements_for(&schema, Dialect::Sqlite).unwrap()[1..],
            [
                "CREATE INDEX IF NOT EXISTS index_documents_on_payload_gin ON documents (payload)",
                "CREATE INDEX IF NOT EXISTS index_documents_on_slug_btree ON documents (slug)",
                "CREATE INDEX IF NOT EXISTS index_documents_on_slug_custom ON documents (slug)",
            ]
        );
    }

    #[test]
    fn a_dynamic_index_method_is_an_ingest_error() {
        let source = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.string "body"
    t.index ["body"], using: index_method
  end
end
"#;
        let error = ingest_schema(source, "db/schema.rb").unwrap_err();
        assert!(error.to_string().contains("must be a literal symbol or string"), "{error}");
    }

    #[test]
    fn schema_rb_folds_method_names_and_rejects_sql_quoting() {
        let source = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.string "body"
    t.index ["body"], using: "GIN"
  end
end
"#;
        let schema = ingest_schema(source, "db/schema.rb").expect("schema");
        assert_eq!(
            schema.tables[&Symbol::from("documents")].indexes[0].using.as_deref(),
            Some("gin")
        );

        let invalid = br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "documents", force: :cascade do |t|
    t.string "body"
    t.index ["body"], using: "odd\"method"
  end
end
"#;
        let error = ingest_schema(invalid, "db/schema.rb").unwrap_err();
        assert!(error.to_string().contains("ASCII unquoted SQL identifier"), "{error}");
    }

    /// An integer's `limit:` is a byte size: solid_cache's `key_hash`
    /// (`limit: 8`) is a `bigint` on Postgres, as Rails creates it. A
    /// 64-bit hash does not fit the `integer` it used to render.
    #[test]
    fn postgres_renders_an_eight_byte_integer_as_bigint() {
        let out = statements(
            r#"ActiveRecord::Schema[7.1].define(version: 1) do
  create_table "solid_cache_entries", force: :cascade do |t|
    t.binary "key", limit: 1024, null: false
    t.integer "key_hash", limit: 8, null: false
    t.integer "byte_size", limit: 4, null: false
  end
end
"#,
            Dialect::Postgres,
        );
        assert_eq!(
            out[0],
            "CREATE TABLE IF NOT EXISTS \"solid_cache_entries\" (\n  \"id\" bigserial PRIMARY KEY,\n  \
             \"key\" bytea NOT NULL,\n  \"key_hash\" bigint NOT NULL,\n  \"byte_size\" integer NOT NULL\n)"
        );
    }

    /// A partial index keeps its `where:` wherever it decides which rows
    /// the index accepts: on a unique index in both dialects. A
    /// non-unique one keeps it on Postgres only; on SQLite it covers
    /// every row, as before. So does a unique one whose predicate SQLite
    /// can't be trusted to run.
    #[test]
    fn a_partial_index_keeps_its_predicate() {
        let schema_rb = r#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "tokens", force: :cascade do |t|
    t.bigint "user_id", null: false
    t.datetime "revoked_at"
    t.integer "state"
    t.string "kind"
    t.index ["user_id"], name: "index_tokens_on_live_user_id", unique: true, where: "(revoked_at IS NULL)"
    t.index ["state"], name: "index_tokens_on_open_state", where: "(state = ANY (ARRAY[2, 3]))"
    t.index ["user_id"], name: "index_tokens_on_initial_user_id", unique: true, where: "((kind)::text = 'initial'::text)"
  end
end
"#;
        let sqlite = statements(schema_rb, Dialect::Sqlite);
        assert_eq!(
            sqlite[1..],
            [
                "CREATE UNIQUE INDEX IF NOT EXISTS index_tokens_on_live_user_id ON tokens (user_id) \
                 WHERE (revoked_at IS NULL)",
                "CREATE INDEX IF NOT EXISTS index_tokens_on_open_state ON tokens (state)",
                "CREATE UNIQUE INDEX IF NOT EXISTS index_tokens_on_initial_user_id ON tokens (user_id)",
            ]
        );
        assert_eq!(
            statements(schema_rb, Dialect::Postgres)[1..],
            [
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_tokens_on_live_user_id\" ON \"tokens\" (\"user_id\") \
                 WHERE (revoked_at IS NULL)",
                "CREATE INDEX IF NOT EXISTS \"index_tokens_on_open_state\" ON \"tokens\" (\"state\") \
                 WHERE (state = ANY (ARRAY[2, 3]))",
                "CREATE UNIQUE INDEX IF NOT EXISTS \"index_tokens_on_initial_user_id\" ON \"tokens\" (\"user_id\") \
                 WHERE ((kind)::text = 'initial'::text)",
            ]
        );
    }

    /// What SQLite is trusted to run as written: Mastodon's predicates,
    /// over the table's own columns. Postgres casts, arrays, `ANY`,
    /// `ILIKE`, `IS DISTINCT FROM`, `IS UNKNOWN` (SQLite reads `UNKNOWN`
    /// as a column), `LIKE` (case rules differ), functions it lacks,
    /// columns the table doesn't have and Postgres-only operators are
    /// not. A literal's contents don't count either way.
    #[test]
    fn sqlite_reads_alike_only_the_shared_predicate_syntax() {
        let schema = ingest_schema(
            br#"ActiveRecord::Schema[8.1].define(version: 1) do
  create_table "statuses", force: :cascade do |t|
    t.boolean "local"
    t.string "uri"
    t.datetime "deleted_at"
    t.integer "visibility"
    t.bigint "reblog_of_id"
    t.string "name"
    t.string "email"
    t.integer "createdAt"
    t.string "revoked at"
    t.boolean "enabled"
    t.integer "state"
  end
end
"#,
            "db/schema.rb",
        )
        .unwrap();
        let table = &schema.tables[&Symbol::from("statuses")];
        for portable in [
            "(uri IS NOT NULL)",
            "((local OR (uri IS NULL)) AND (deleted_at IS NULL) AND (visibility = 0) AND (reblog_of_id IS NULL))",
            "(enabled = false)",
            "(name <> 'it''s a :: b [1] UNKNOWN')",
            "(\"createdAt\" > 0) AND (lower(email) || 'x' != '')",
            "(\"revoked at\" IS NULL)",
            "(state IN (1, 2)) AND (visibility BETWEEN -1 AND 2.5)",
        ] {
            assert!(sqlite_reads_alike(portable, table), "{portable}");
        }
        for postgres_only in [
            "((name)::text = 'initial'::text)",
            "(state = ANY (ARRAY[2, 3]))",
            "(name ILIKE 'a%')",
            "(name LIKE 'a%')",
            "(name IS DISTINCT FROM email)",
            "(enabled IS UNKNOWN)",
            "(num_nonnulls(uri, email) > 0)",
            "(archived_at IS NULL)",
            "(\"revoked_at\" IS NULL)",
            "(statuses.uri IS NULL)",
            "(name ~~ 'a%')",
            "(name = 'unterminated)",
        ] {
            assert!(!sqlite_reads_alike(postgres_only, table), "{postgres_only}");
        }
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
            (ColumnType::Json, "json"),
            (ColumnType::Jsonb, "jsonb"),
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
            vec![Index {
                name: Symbol::from("index_user_on_createdAt"),
                columns: vec![Symbol::from("createdAt")],
                unique: true,
                using: None,
                predicate: None,
            }],
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
