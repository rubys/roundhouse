//! `db/structure.sql` — parse a `pg_dump`-shaped Postgres DDL dump into
//! the same target-neutral `Schema` that [`super::schema::ingest_schema`]
//! produces from `db/schema.rb`.
//!
//! Rails apps with `config.active_record.schema_format = :sql` never
//! write `schema.rb`; `rails db:schema:dump` writes `db/structure.sql`
//! instead (raw `pg_dump` output for Postgres). This module is a
//! hand-rolled statement-level parser over that dump — no new crate
//! dependency, no general SQL grammar, just the handful of DDL shapes
//! the Rails schema dumper's own output space actually uses plus the
//! handful more that a real `pg_dump` adds around them (schemas,
//! extensions, functions, triggers, views, partitioning).
//!
//! The parser is a single left-to-right pass over top-level statements
//! (split on `;`, `$$`/`$tag$`-aware so a `plpgsql` function body's
//! internal semicolons don't fragment it, quote-aware so a string
//! literal's punctuation is never mistaken for SQL syntax). Statements
//! are dispatched by their opening keywords; anything with no Rails
//! `schema.rb` equivalent (`CREATE FUNCTION`, `CREATE TRIGGER`, `SET`,
//! `COMMENT ON`, …) is skipped silently, and anything genuinely
//! unrecognized is ledgered once per distinct statement head so a
//! `--continue` survey run stays honest about what fell through.
//!
//! Column types that have no `ColumnType` mapping are ledgered through
//! the exact same "column dropped" mechanism `ingest_schema` uses
//! (`src/ingest/schema.rs`) — never a silent drop, since a dropped
//! column's index would still be emitted and the DDL would not apply.
//! Composite (multi-column) primary keys and foreign keys — routine in
//! this dialect, since Postgres has no trouble with them, unlike the
//! single-column `Column`/`ForeignKey` IR — are ledgered the same way
//! rather than silently narrowed to one column.

use std::collections::HashSet;

use crate::schema::generated::GeneratedExpressionDialect;
use crate::schema::{
    Column, ColumnType, ForeignKey, GeneratedColumn, GeneratedColumnStorage, Index,
    ReferentialAction, Schema, Table,
};
use crate::{Symbol, TableRef};

use super::{IngestError, IngestResult};

/// Ingest a structure dump with the Portable generated-expression grammar.
/// PostgreSQL-only casts require the explicit DDL-dialect entry point.
pub fn ingest_structure_sql(source: &[u8], file: &str) -> IngestResult<Schema> {
    ingest_structure_sql_with_generated_expression_dialect(
        source,
        file,
        GeneratedExpressionDialect::Portable,
    )
}

/// Ingest a PostgreSQL structure dump with an explicit generated-expression
/// grammar. The default [`ingest_structure_sql`] remains Portable for app
/// analysis and current target emission.
pub fn ingest_structure_sql_with_generated_expression_dialect(
    source: &[u8],
    file: &str,
    dialect: GeneratedExpressionDialect,
) -> IngestResult<Schema> {
    let text = String::from_utf8_lossy(source).into_owned();
    super::sources::register(file, &text);

    let mut schema = Schema::default();
    let mut enum_types: HashSet<String> = HashSet::new();
    // Columns, table-level constraints, and whole statements the walk
    // could not model. Never silent — see the module header.
    let mut gaps: Vec<IngestError> = Vec::new();
    // Caps the catch-all "statement not modeled" ledger to one entry
    // per distinct statement head, not per occurrence (a 200k-line dump
    // can repeat the same unmodeled ALTER INDEX shape thousands of
    // times).
    let mut seen_heads: HashSet<String> = HashSet::new();

    for raw in top_level_split(&text, b';') {
        let stmt = strip_leading_comment_banner(&raw);
        if stmt.is_empty() {
            continue;
        }
        dispatch_statement(
            stmt,
            file,
            &mut schema,
            &mut enum_types,
            &mut gaps,
            &mut seen_heads,
            dialect,
        );
    }

    if !gaps.is_empty() {
        if super::survey::is_active() {
            for gap in &gaps {
                super::survey::record(gap);
            }
        } else {
            return Err(gaps.swap_remove(0));
        }
    }
    Ok(schema)
}

// ---------------------------------------------------------------------
// Statement dispatch
// ---------------------------------------------------------------------

/// Dispatch one top-level dump statement to its schema handler.
fn dispatch_statement(
    stmt: &str,
    file: &str,
    schema: &mut Schema,
    enum_types: &mut HashSet<String>,
    gaps: &mut Vec<IngestError>,
    seen_heads: &mut HashSet<String>,
    dialect: GeneratedExpressionDialect,
) {
    // A child partition attached via `ALTER TABLE … ATTACH PARTITION
    // …` rather than declared `PARTITION OF` up front (the shape this
    // dialect's real dumps actually use — see the module header):
    // the child got its own independent `CREATE TABLE` earlier in the
    // dump (same columns as the parent, duplicated), which this now
    // retracts. Modeling 32 identical shards of `financial_line_items`
    // as 32 separate tables would be noise, not signal: same columns,
    // same composite key, 32x the ledger entries for zero new
    // information. `ALTER INDEX … ATTACH PARTITION …` (a partitioned
    // index's own child-index attachment) has no table to retract —
    // skipped, same as before.
    if starts_with_ci(stmt, "ALTER TABLE") && stmt.to_ascii_uppercase().contains("ATTACH PARTITION") {
        handle_attach_partition(stmt, schema);
        return;
    }
    if stmt.to_ascii_uppercase().contains("ATTACH PARTITION") {
        return;
    }

    if starts_with_ci(stmt, "CREATE TABLE") {
        handle_create_table(stmt, file, schema, enum_types, gaps, dialect);
        return;
    }
    if starts_with_ci(stmt, "CREATE MATERIALIZED VIEW")
        || starts_with_ci(stmt, "CREATE VIEW")
        || starts_with_ci(stmt, "CREATE OR REPLACE VIEW")
    {
        handle_create_view(stmt, file, gaps);
        return;
    }
    if starts_with_ci(stmt, "CREATE TYPE") {
        maybe_register_enum(stmt, enum_types);
        return;
    }
    if starts_with_ci(stmt, "CREATE UNIQUE INDEX") || starts_with_ci(stmt, "CREATE INDEX") {
        handle_create_index(stmt, file, schema, gaps);
        return;
    }
    if starts_with_ci(stmt, "ALTER TABLE") {
        handle_alter_table(stmt, file, schema, gaps, seen_heads);
        return;
    }

    // No `schema.rb` equivalent for any of these — skipped silently,
    // same as the schema.rb walker skips `SET`/`SELECT`/etc. it never
    // sees in the first place.
    const SILENT_PREFIXES: &[&str] = &[
        "CREATE OR REPLACE FUNCTION",
        "CREATE FUNCTION",
        "CREATE TRIGGER",
        "CREATE SEQUENCE",
        "ALTER SEQUENCE",
        "CREATE EXTENSION",
        "CREATE SCHEMA",
        "COMMENT ON",
        "GRANT",
        "REVOKE",
        "SET",
        "SELECT",
        "ALTER FUNCTION",
        "ALTER DEFAULT PRIVILEGES",
        "CREATE OPERATOR",
        "CREATE AGGREGATE",
        "CREATE CAST",
        "CREATE DOMAIN",
        "CREATE RULE",
        "SECURITY LABEL",
        "CREATE PUBLICATION",
        "ALTER PUBLICATION",
        "CREATE COLLATION",
        "CREATE SERVER",
        "CREATE FOREIGN",
        "ALTER INDEX",
        "DROP",
        "INSERT INTO",
        // Found in Procore's real dump, not anticipated up front: event
        // triggers, extended-statistics objects, and full-text-search
        // configuration — none has a `schema.rb` equivalent either.
        "CREATE EVENT TRIGGER",
        "ALTER EVENT TRIGGER",
        "CREATE STATISTICS",
        "ALTER STATISTICS",
        "CREATE TEXT SEARCH CONFIGURATION",
        "ALTER TEXT SEARCH CONFIGURATION",
        "CREATE TEXT SEARCH DICTIONARY",
        "CREATE TEXT SEARCH PARSER",
        "CREATE TEXT SEARCH TEMPLATE",
    ];
    if SILENT_PREFIXES.iter().any(|p| starts_with_ci(stmt, p)) {
        return;
    }

    let head = stmt.split_whitespace().take(2).collect::<Vec<_>>().join(" ").to_ascii_uppercase();
    if head.is_empty() {
        return;
    }
    record_unmodeled(gaps, seen_heads, file, &head);
}

fn record_unmodeled(gaps: &mut Vec<IngestError>, seen_heads: &mut HashSet<String>, file: &str, head: &str) {
    if seen_heads.insert(head.to_string()) {
        gaps.push(IngestError::Unsupported {
            file: file.into(),
            message: format!("structure.sql statement not modeled: {head}"),
        });
    }
}

// ---------------------------------------------------------------------
// CREATE TABLE
// ---------------------------------------------------------------------

/// Parse one `CREATE TABLE` and validate generated expressions under `dialect`.
fn handle_create_table(
    stmt: &str,
    file: &str,
    schema: &mut Schema,
    enum_types: &HashSet<String>,
    gaps: &mut Vec<IngestError>,
    dialect: GeneratedExpressionDialect,
) {
    // `CREATE TABLE child PARTITION OF parent FOR VALUES …` — the
    // parent's own CREATE TABLE already carries the columns, and this
    // form has no column list of its own to model. (The other shape
    // Postgres allows — an independent `CREATE TABLE` for the child,
    // later joined to the parent via `ALTER TABLE … ATTACH PARTITION
    // …` — DOES have its own column list here, so it's ingested like
    // any other table and then retracted by `handle_attach_partition`
    // once the ATTACH statement names it.)
    if stmt.to_ascii_uppercase().contains("PARTITION OF") {
        return;
    }

    let Some(mut rest) = strip_prefix_ci(stmt, "CREATE TABLE") else { return };
    rest = rest.trim_start();
    if let Some(r2) = strip_prefix_ci(rest, "IF NOT EXISTS") {
        rest = r2.trim_start();
    }
    let Some((table_name, consumed)) = read_ident(rest, 0) else { return };
    // Rails' own bookkeeping tables — schema.rb never lists them
    // either.
    if table_name.eq_ignore_ascii_case("schema_migrations")
        || table_name.eq_ignore_ascii_case("ar_internal_metadata")
    {
        return;
    }

    let after_name = &rest[consumed..];
    let Some(open) = find_first_open_paren(after_name, 0) else { return };
    let Some(close) = matching_close_paren(after_name, open) else { return };
    let body = &after_name[open + 1..close];

    let mut columns: Vec<Column> = Vec::new();
    for seg in top_level_split(body, b',') {
        let seg = seg.trim();
        if seg.is_empty() {
            continue;
        }
        if is_table_level_constraint(seg) {
            handle_inline_table_constraint(seg, &table_name, &mut columns, gaps, file);
            continue;
        }
        match parse_column_def(seg, &table_name, file, enum_types) {
            Ok(Some(col)) => columns.push(col),
            Ok(None) => {}
            Err(gap) => gaps.push(gap),
        }
    }

    // Everything after the closing paren (`PARTITION BY …`, `INHERITS
    // (…)`, `WITH (…)` storage params) is intentionally never
    // inspected — none of it changes the column list, and schema.rb
    // has no equivalent to round-trip it into anyway.
    let mut table = Table {
        name: Symbol::from(table_name.as_str()),
        columns,
        indexes: Vec::new(),
        foreign_keys: Vec::new(),
        virtual_module: None,
    };
    let invalid_generated = crate::schema::generated::validate_table_with_dialect(&table, dialect);
    for (column, reason) in &invalid_generated {
        gaps.push(IngestError::Unsupported {
            file: file.into(),
            message: format!("generated column dropped: {table_name}.{column}: {reason}"),
        });
    }
    if !invalid_generated.is_empty() {
        table.columns.retain(|col| {
            !invalid_generated.iter().any(|(name, _)| name == col.name.as_str())
        });
    }
    schema.tables.insert(Symbol::from(table_name), table);
}

/// A comma-separated entry inside `CREATE TABLE (...)` that is a
/// table-level constraint rather than a column definition. The keyword
/// is a whole word: Postgres dumps column names unquoted, and columns
/// named `checkout_id`, `unique_id` or `excluded_flag` were read as
/// `CHECK`/`UNIQUE`/`EXCLUDE` constraints and dropped.
fn is_table_level_constraint(seg: &str) -> bool {
    const KW: &[&str] = &["CONSTRAINT", "PRIMARY KEY", "UNIQUE", "CHECK", "EXCLUDE", "FOREIGN KEY"];
    KW.iter().any(|k| {
        starts_with_ci(seg, k)
            && !seg[k.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// `[CONSTRAINT name] PRIMARY KEY (cols)` inside the column-list parens
/// sets the primary-key flag on the matching column(s); every other
/// table-level constraint form (`UNIQUE`, `CHECK`, `EXCLUDE`, an
/// inline `FOREIGN KEY`) is recognized and skipped — it costs the DDL
/// nothing schema.rb would have captured either.
fn handle_inline_table_constraint(
    seg: &str,
    table_name: &str,
    columns: &mut [Column],
    gaps: &mut Vec<IngestError>,
    file: &str,
) {
    let words = top_level_words(seg);
    let Some(pos) = word_seq_pos_after(&words, 0, &["PRIMARY", "KEY"]) else { return };
    let Some(open) = find_first_open_paren(seg, pos) else { return };
    let Some(close) = matching_close_paren(seg, open) else { return };
    let cols: Vec<String> = top_level_split(&seg[open + 1..close], b',')
        .iter()
        .filter_map(|c| read_ident(c.trim(), 0).map(|(n, _)| n))
        .collect();
    match cols.len() {
        0 => {}
        1 => {
            if let Some(col) = columns.iter_mut().find(|c| c.name.as_str() == cols[0]) {
                col.primary_key = true;
            }
        }
        _ => gaps.push(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "primary key dropped: composite key ({table_name}({}))",
                cols.join(", ")
            ),
        }),
    }
}

/// One column-definition entry from inside `CREATE TABLE (...)`.
/// `Ok(None)` for a malformed/empty entry (defensive — every real
/// entry is either this or a table-level constraint, already routed
/// away by [`is_table_level_constraint`]); `Err` when the entry names
/// a column but its type has no `ColumnType` mapping.
fn parse_column_def(
    seg: &str,
    table: &str,
    file: &str,
    enum_types: &HashSet<String>,
) -> Result<Option<Column>, IngestError> {
    let Some((col_name, after_name)) = read_ident(seg, 0) else { return Ok(None) };
    let rest = seg[after_name..].trim_start();
    if rest.is_empty() {
        return Ok(None);
    }

    let mod_start = find_modifier_start(rest);
    let type_phrase = rest[..mod_start].trim();
    let modifiers = &rest[mod_start..];
    if type_phrase.is_empty() {
        return Ok(None);
    }

    let generated = parse_generated_modifier(modifiers).map_err(|message| IngestError::Unsupported {
        file: file.into(),
        message: format!("generated column dropped: {table}.{col_name}: {message}"),
    })?;
    let (nullable, default) = parse_modifiers(modifiers);
    if generated.is_some() && default.is_some() {
        return Err(unsupported_col(file, table, &col_name, "generated column also has DEFAULT"));
    }
    let col_type = resolve_column_type(type_phrase, enum_types)
        .ok_or_else(|| unsupported_col(file, table, &col_name, type_phrase))?;
    let generated_text_compatible = has_nonportable_text_source_type(type_phrase, enum_types)
        .then_some(false);
    let generated_int4_compatible =
        has_nonportable_int4_source_type(type_phrase).then_some(false);

    Ok(Some(Column {
        name: Symbol::from(col_name),
        col_type,
        nullable,
        default,
        primary_key: false,
        generated,
        generated_text_compatible,
        generated_int4_compatible,
    }))
}

/// Keep only the negative type provenance that `ColumnType` cannot
/// express. Ordinary app typing deliberately strips schema qualifiers,
/// so a custom qualified domain named `text`, `varchar`, `json`, or
/// `jsonb` can normalize to a built-in-looking `ColumnType`. Generated
/// expressions admit only bare built-ins or the real `pg_catalog` names
/// `text`, `varchar`, `json`, and `jsonb`; in particular,
/// `pg_catalog.character varying` is not a qualified built-in spelling.
/// Quoted qualified names are also rejected because type normalization
/// lowercases their contents without preserving SQL identifier case.
/// Other aliases (`inet`, `interval`, enums, `citext`, and fixed
/// `character`) also normalize to text-like types without portable text
/// semantics. A text typmod is marked because this IR does not retain it.
fn has_nonportable_text_source_type(
    type_phrase: &str,
    enum_types: &HashSet<String>,
) -> bool {
    let (base, first_type_modifier, _) = strip_parens_capture_nums(type_phrase);
    let (qualifier, base_name) = base
        .rsplit_once('.')
        .map(|(schema, name)| (Some(schema), name))
        .unwrap_or((None, base.as_str()));
    let text_or_json_alias = matches!(
        base_name,
        "text" | "varchar" | "character varying" | "character" | "char" | "bpchar"
            | "citext" | "json" | "jsonb" | "inet" | "cidr" | "macaddr"
            | "macaddr8" | "interval"
    );
    let qualified_non_builtin = qualifier.is_some_and(|schema| {
        !schema.eq_ignore_ascii_case("pg_catalog")
            || !matches!(base_name, "text" | "varchar" | "json" | "jsonb")
    });
    matches!(
        base_name,
        "inet" | "cidr" | "macaddr" | "macaddr8" | "interval" | "character" | "char" | "bpchar" | "citext"
    ) || enum_types.contains(base_name)
        || (base_name == "text" && first_type_modifier.is_some())
        || (text_or_json_alias && qualified_non_builtin)
}

/// Preserve the negative integer-width evidence that `ColumnType::Integer`
/// cannot express. PostgreSQL `smallint`/`int2`, sequence-backed `serial`
/// aliases, and integer typmods are not exact int4 results. Unqualified
/// `integer`/`int`/`int4` spellings are accepted by the SQL grammar,
/// but only `pg_catalog.int4` is a valid qualified catalog spelling.
fn has_nonportable_int4_source_type(type_phrase: &str) -> bool {
    let (source_type, _, _) = strip_parens_capture_nums(type_phrase);
    let (qualifier, base) = match source_type.rsplit_once('.') {
        Some((schema, name)) => (Some(schema), name),
        None => (None, source_type.as_str()),
    };
    if matches!(base, "smallint" | "int2" | "serial" | "serial4") {
        return true;
    }
    if matches!(base, "integer" | "int") {
        // PostgreSQL has no qualified type names for these SQL grammar
        // aliases, including under pg_catalog.
        return qualifier.is_some() || type_phrase.contains('(');
    }
    if base == "int4" {
        return type_phrase.contains('(')
            || qualifier.is_some_and(|schema| !schema.eq_ignore_ascii_case("pg_catalog"));
    }
    false
}

fn unsupported_col(file: &str, table: &str, col: &str, type_name: &str) -> IngestError {
    IngestError::Unsupported {
        file: file.into(),
        message: format!("column dropped: {table}.{col} has unsupported type `{type_name}`"),
    }
}

/// A Postgres type phrase (e.g. `character varying(255)`, `timestamp(6)
/// without time zone`, `numeric(20,2)`, `public.widget_status`) to its
/// `ColumnType`, mirroring exactly what `ingest_schema::column_with_type`
/// maps for the equivalent `schema.rb` type name (see
/// `src/ingest/schema.rs`) — including that mapping's own known limits:
/// `numeric`/`decimal` never carry precision/scale (schema.rb ingest
/// never captures them either, so this stays parity rather than a new
/// capability), and array columns are never coerced to their base type
/// (there is no `array: true` notion on `ColumnType`). Enum-typed
/// columns (a name registered by a preceding `CREATE TYPE … AS ENUM`)
/// map to `String`, same as schema.rb's own `enum` → `String`. Returns
/// `None` for anything with no mapping — the caller ledgers that as a
/// dropped column, never a silent one.
fn resolve_column_type(type_phrase: &str, enum_types: &HashSet<String>) -> Option<ColumnType> {
    let (mut base, num1, _num2) = strip_parens_capture_nums(type_phrase);
    let is_array = base.ends_with("[]");
    if is_array {
        base.truncate(base.len() - 2);
        base = base.trim_end().to_string();
    }
    if let Some(dot) = base.rfind('.') {
        base = base[dot + 1..].to_string();
    }
    if is_array {
        // `text[]`, `bigint[]`, … — no array notion on `ColumnType`;
        // ledgered by the caller rather than silently scalarized.
        return None;
    }

    let limit = || num1.and_then(|n| u32::try_from(n).ok());
    Some(match base.as_str() {
        "integer" | "int" | "int4" | "smallint" | "int2" => ColumnType::Integer,
        "bigint" | "int8" | "bigserial" | "serial8" => ColumnType::BigInt,
        "serial" | "serial4" => ColumnType::Integer,
        "boolean" | "bool" => ColumnType::Boolean,
        "character varying" | "varchar" | "character" | "char" | "bpchar" => {
            ColumnType::String { limit: limit() }
        }
        "text" | "citext" => ColumnType::Text,
        "date" => ColumnType::Date,
        "timestamp without time zone" | "timestamp with time zone" | "timestamptz" | "timestamp" => {
            ColumnType::DateTime
        }
        "time without time zone" | "time with time zone" | "timetz" | "time" => ColumnType::Time,
        "numeric" | "decimal" => ColumnType::Decimal { precision: None, scale: None },
        "double precision" | "real" | "float4" | "float8" | "float" => ColumnType::Float,
        "bytea" => ColumnType::Binary,
        "json" => ColumnType::Json,
        "jsonb" => ColumnType::Jsonb,
        "uuid" => ColumnType::Uuid,
        "inet" | "cidr" | "macaddr" | "macaddr8" => ColumnType::String { limit: None },
        "interval" => ColumnType::String { limit: None },
        other if enum_types.contains(other) => ColumnType::String { limit: None },
        _ => return None,
    })
}

// ---------------------------------------------------------------------
// CREATE VIEW / CREATE TYPE
// ---------------------------------------------------------------------

/// `CREATE [MATERIALIZED] VIEW name AS SELECT …` — not modeled (no SQL
/// SELECT parser here); ledgered once per view so a model backed by
/// one is a visible, not silent, gap.
fn handle_create_view(stmt: &str, file: &str, gaps: &mut Vec<IngestError>) {
    let rest = strip_prefix_ci(stmt, "CREATE MATERIALIZED VIEW")
        .or_else(|| strip_prefix_ci(stmt, "CREATE OR REPLACE VIEW"))
        .or_else(|| strip_prefix_ci(stmt, "CREATE VIEW"));
    let Some(rest) = rest else { return };
    let Some((name, _)) = read_ident(rest.trim_start(), 0) else { return };
    gaps.push(IngestError::Unsupported {
        file: file.into(),
        message: format!("view not modeled: a model backed by this view has no columns ({name})"),
    });
}

/// `CREATE TYPE name AS ENUM (…)` registers the (schema-stripped,
/// lowercased) type name so a later column declared with it resolves
/// to `ColumnType::String` — the same target schema.rb's own `enum`
/// column type maps to. A non-enum `CREATE TYPE` (a composite type
/// like the `uniform_agg_state_*` aggregate-state types in Procore's
/// dump, or a domain) has no Rails equivalent and is otherwise
/// ignored; if one is later used as a column type, `resolve_column_type`
/// ledgers that column as unsupported same as any other unknown type.
fn maybe_register_enum(stmt: &str, enum_types: &mut HashSet<String>) {
    let Some(rest) = strip_prefix_ci(stmt, "CREATE TYPE") else { return };
    let rest = rest.trim_start();
    let Some((name, consumed)) = read_ident(rest, 0) else { return };
    let after = rest[consumed..].trim_start();
    if strip_prefix_ci(after, "AS ENUM").is_some() {
        enum_types.insert(name.to_ascii_lowercase());
    }
}

// ---------------------------------------------------------------------
// CREATE INDEX
// ---------------------------------------------------------------------

/// `CREATE [UNIQUE] INDEX [name] ON table [USING method] (cols) [WHERE
/// …]`. The `WHERE` predicate is kept as written, as schema.rb's
/// `where:` is: on a unique index it limits which rows must be
/// distinct. An expression column (`lower(name)`, `COALESCE(…)`, an
/// operator expression) has no
/// `Symbol` to hold it, so the whole index is skipped rather than
/// ledgered: it costs the DDL nothing (indexes don't feed column
/// typing) and ledgering one line per such index would swamp the
/// survey report with a purely-cosmetic gap (this dump alone has
/// several thousand plain indexes and only a handful of expression
/// ones).
fn handle_create_index(stmt: &str, file: &str, schema: &mut Schema, gaps: &mut Vec<IngestError>) {
    let Some(mut rest) = strip_prefix_ci(stmt, "CREATE") else { return };
    rest = rest.trim_start();
    let unique = if let Some(r2) = strip_prefix_ci(rest, "UNIQUE") {
        rest = r2.trim_start();
        true
    } else {
        false
    };
    let Some(r3) = strip_prefix_ci(rest, "INDEX") else { return };
    rest = r3.trim_start();
    if let Some(r4) = strip_prefix_ci(rest, "CONCURRENTLY") {
        rest = r4.trim_start();
    }
    if let Some(r4) = strip_prefix_ci(rest, "IF NOT EXISTS") {
        rest = r4.trim_start();
    }
    let Some((index_name, consumed)) = read_ident(rest, 0) else { return };
    rest = rest[consumed..].trim_start();
    let Some(r5) = strip_prefix_ci(rest, "ON") else { return };
    rest = r5.trim_start();
    let Some((table_name, consumed2)) = read_ident(rest, 0) else { return };
    let table_sym = Symbol::from(table_name);
    let after_table = &rest[consumed2..];

    let Some(open) = find_first_open_paren(after_table, 0) else { return };
    let access_clause = &after_table[..open];
    let access_start = skip_sql_trivia(access_clause, 0);
    let using = if access_start == access_clause.len() {
        None
    } else {
        let Some(method_start) = consume_sql_keyword_ci(access_clause, access_start, "USING")
        else {
            gaps.push(IngestError::Unsupported {
                file: file.into(),
                message: "CREATE INDEX clause before the column list is not modeled".into(),
            });
            return;
        };
        let method_start = skip_sql_trivia(access_clause, method_start);
        let method = &access_clause[method_start..];
        let quoted = method.starts_with('"');
        match read_index_access_method(method) {
            Some((name, consumed))
                if !name.is_empty()
                    && skip_sql_trivia(access_clause, method_start + consumed)
                        == access_clause.len() =>
            {
                Some(if quoted { name } else { name.to_ascii_lowercase() })
            }
            _ => {
                gaps.push(IngestError::Unsupported {
                    file: file.into(),
                    message: "CREATE INDEX access method is not a SQL identifier".into(),
                });
                return;
            }
        }
    };
    let Some(close) = matching_close_paren(after_table, open) else { return };
    let body = &after_table[open + 1..close];

    let mut cols: Vec<Symbol> = Vec::new();
    for entry in top_level_split(body, b',') {
        let e = entry.trim();
        if e.is_empty() {
            continue;
        }
        match read_ident(e, 0) {
            Some((name, consumed3)) if e[consumed3..].trim().is_empty() => cols.push(Symbol::from(name)),
            _ => return, // expression column — see doc comment above
        }
    }
    if cols.is_empty() {
        return;
    }

    // pg_dump puts the predicate last, after any `INCLUDE`, `NULLS NOT
    // DISTINCT`, `WITH` or `TABLESPACE`, so it is the rest of the
    // statement.
    let words = top_level_words(after_table);
    let predicate = word_seq_pos_after(&words, close, &["WHERE"])
        .map(|pos| after_table[pos + "WHERE".len()..].trim().to_string())
        .filter(|p| !p.is_empty());

    if let Some(table) = schema.tables.get_mut(&table_sym) {
        // An index over a column the walk dropped cannot apply — same
        // retain-filter `ingest_schema` uses (see `table_from_create_table`).
        if cols.iter().all(|c| table.columns.iter().any(|col| col.name == *c)) {
            table.indexes.push(Index {
                name: Symbol::from(index_name),
                columns: cols,
                unique,
                using,
                predicate,
            });
        }
    }
}

/// `ALTER TABLE [ONLY] parent ATTACH PARTITION child FOR VALUES …` —
/// removes `child` from `schema.tables` if it's there. `child` always
/// got its own `CREATE TABLE` earlier in the dump (pg_dump orders by
/// dependency), so by the time this statement is reached the entry
/// exists to remove; a no-op otherwise (defensive — e.g. if a future
/// dump ever used `PARTITION OF` for the same child, `handle_create_table`
/// would already have skipped it and there'd be nothing here to find).
fn handle_attach_partition(stmt: &str, schema: &mut Schema) {
    let words = top_level_words(stmt);
    let Some(pos) = word_seq_pos_after(&words, 0, &["ATTACH", "PARTITION"]) else { return };
    let name_start = pos + "ATTACH PARTITION".len();
    let Some((child_name, _)) = read_ident(stmt, name_start) else { return };
    schema.tables.shift_remove(&Symbol::from(child_name));
}

// ---------------------------------------------------------------------
// ALTER TABLE
// ---------------------------------------------------------------------

fn handle_alter_table(
    stmt: &str,
    file: &str,
    schema: &mut Schema,
    gaps: &mut Vec<IngestError>,
    seen_heads: &mut HashSet<String>,
) {
    let Some(mut rest) = strip_prefix_ci(stmt, "ALTER TABLE") else { return };
    rest = rest.trim_start();
    if let Some(r2) = strip_prefix_ci(rest, "ONLY") {
        rest = r2.trim_start();
    }
    let Some((table_name, consumed)) = read_ident(rest, 0) else { return };
    let table_sym = Symbol::from(table_name.clone());
    let after = &rest[consumed..];
    let words = top_level_words(after);

    if let Some(pos) = word_seq_pos_after(&words, 0, &["PRIMARY", "KEY"]) {
        handle_add_primary_key(after, pos, &table_name, table_sym, schema, gaps, file);
        return;
    }
    if let Some(pos) = word_seq_pos_after(&words, 0, &["FOREIGN", "KEY"]) {
        handle_add_foreign_key(after, &words, pos, &table_name, table_sym, schema, gaps, file);
        return;
    }
    if word_seq_pos_after(&words, 0, &["ALTER", "COLUMN"]).is_some()
        && word_seq_pos_after(&words, 0, &["SET", "DEFAULT"]).is_some()
    {
        handle_alter_column_default(after, &words, table_sym, schema);
        return;
    }

    // `REPLICA IDENTITY FULL`, `OWNER TO`, `CLUSTER ON`, `VALIDATE
    // CONSTRAINT`, … — no schema.rb equivalent, but unlike the
    // explicitly-silent statement heads above, this IS a form of
    // `ALTER TABLE` we haven't taught the walk, so it goes through the
    // catch-all (bucketed under the two-word head, same as any other
    // unrecognized statement).
    record_unmodeled(gaps, seen_heads, file, "ALTER TABLE");
}

/// `ADD CONSTRAINT [name] PRIMARY KEY (cols)`. A composite key is a
/// real shape in this dialect (Postgres partitioned tables routinely
/// key on `(id, partition_key)`) that the single-column `Column.
/// primary_key` flag cannot hold — ledgered rather than narrowed to
/// one column, which would silently misrepresent the table's key.
fn handle_add_primary_key(
    after: &str,
    pk_pos: usize,
    table_name: &str,
    table_sym: Symbol,
    schema: &mut Schema,
    gaps: &mut Vec<IngestError>,
    file: &str,
) {
    // A retracted partition shard (see `handle_attach_partition`) has
    // no table entry left to set a pk flag on — and, just as
    // importantly, no business ledgering a composite-key gap for a
    // table this Schema no longer models at all. Its own `ADD
    // CONSTRAINT ... PRIMARY KEY` statement always comes AFTER its
    // `ATTACH PARTITION` in a real dump (pg_dump orders constraints
    // after the table's attached-into-parent state), so this is not a
    // race — the removal has already happened by the time we get here.
    if !schema.tables.contains_key(&table_sym) {
        return;
    }
    let Some(open) = find_first_open_paren(after, pk_pos) else { return };
    let Some(close) = matching_close_paren(after, open) else { return };
    let cols: Vec<String> = top_level_split(&after[open + 1..close], b',')
        .iter()
        .filter_map(|c| read_ident(c.trim(), 0).map(|(n, _)| n))
        .collect();
    match cols.len() {
        0 => {}
        1 => {
            if let Some(table) = schema.tables.get_mut(&table_sym) {
                if let Some(col) = table.columns.iter_mut().find(|c| c.name.as_str() == cols[0]) {
                    col.primary_key = true;
                }
            }
        }
        _ => gaps.push(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "primary key dropped: composite key ({table_name}({}))",
                cols.join(", ")
            ),
        }),
    }
}

/// `ADD CONSTRAINT [name] FOREIGN KEY (from_cols) REFERENCES
/// ref_table(ref_cols) [ON DELETE action] [ON UPDATE action]`. Same
/// composite-key rationale as [`handle_add_primary_key`]: a
/// multi-column FK (very common in this dialect — `security_root_*_fk
/// FOREIGN KEY (project_id, company_id) REFERENCES projects(id,
/// company_id)` is a real Procore shape) is ledgered rather than
/// narrowed to its first column pair.
fn handle_add_foreign_key(
    after: &str,
    words: &[(usize, String)],
    fk_pos: usize,
    table_name: &str,
    table_sym: Symbol,
    schema: &mut Schema,
    gaps: &mut Vec<IngestError>,
    file: &str,
) {
    // Same retracted-partition-shard guard as `handle_add_primary_key`.
    if !schema.tables.contains_key(&table_sym) {
        return;
    }
    let Some(open1) = find_first_open_paren(after, fk_pos) else { return };
    let Some(close1) = matching_close_paren(after, open1) else { return };
    let from_cols: Vec<String> = top_level_split(&after[open1 + 1..close1], b',')
        .iter()
        .filter_map(|c| read_ident(c.trim(), 0).map(|(n, _)| n))
        .collect();

    let Some(refs_pos) = word_seq_pos_after(words, close1, &["REFERENCES"]) else { return };
    let ref_name_start = refs_pos + "REFERENCES".len();
    let Some((ref_table_raw, ref_name_end)) = read_ident(after, ref_name_start) else { return };
    let Some(open2) = find_first_open_paren(after, ref_name_end) else { return };
    let Some(close2) = matching_close_paren(after, open2) else { return };
    let to_cols: Vec<String> = top_level_split(&after[open2 + 1..close2], b',')
        .iter()
        .filter_map(|c| read_ident(c.trim(), 0).map(|(n, _)| n))
        .collect();

    if from_cols.len() == 1 && to_cols.len() == 1 {
        let on_delete = action_after(words, close2, "DELETE");
        let on_update = action_after(words, close2, "UPDATE");
        if let Some(table) = schema.tables.get_mut(&table_sym) {
            table.foreign_keys.push(ForeignKey {
                from_column: Symbol::from(from_cols[0].clone()),
                to_table: TableRef(Symbol::from(ref_table_raw)),
                to_column: Symbol::from(to_cols[0].clone()),
                on_delete,
                on_update,
            });
        }
    } else {
        gaps.push(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "foreign key dropped: composite key ({table_name}({}) -> {ref_table_raw}({}))",
                from_cols.join(", "),
                to_cols.join(", ")
            ),
        });
    }
}

/// `ON DELETE <action>` / `ON UPDATE <action>` following a `REFERENCES
/// table(cols)` clause, starting the search at `after_byte` (the end
/// of that clause). Missing or unrecognized → `NoAction`, matching the
/// `ReferentialAction` default.
fn action_after(words: &[(usize, String)], after_byte: usize, kind: &str) -> ReferentialAction {
    use ReferentialAction::*;
    let Some(pos) = word_seq_pos_after(words, after_byte, &["ON", kind]) else { return NoAction };
    let Some(idx) = words.iter().position(|(p, _)| *p == pos) else { return NoAction };
    let w1 = words.get(idx + 2).map(|(_, w)| w.as_str());
    let w2 = words.get(idx + 3).map(|(_, w)| w.as_str());
    match (w1, w2) {
        (Some("CASCADE"), _) => Cascade,
        (Some("RESTRICT"), _) => Restrict,
        (Some("SET"), Some("NULL")) => SetNull,
        (Some("SET"), Some("DEFAULT")) => SetDefault,
        _ => NoAction,
    }
}

/// `ALTER COLUMN c SET DEFAULT <expr>`. Only a quoted string-literal
/// `<expr>` is retained as `Column.default` — parity with schema.rb
/// ingest's own `change_column_default` fold, which likewise keeps
/// only string literals. A `nextval(…)` expression (the shape every
/// occurrence of this statement in a real dump actually has — it is
/// how Postgres marks an identity/serial column's next-value default)
/// is deliberately left uncaptured: it names a sequence, not a schema
/// fact `Column.default: Option<String>` should hold.
fn handle_alter_column_default(after: &str, words: &[(usize, String)], table_sym: Symbol, schema: &mut Schema) {
    let Some(col_pos) = word_seq_pos_after(words, 0, &["ALTER", "COLUMN"]) else { return };
    let name_start = col_pos + "ALTER COLUMN".len();
    let Some((col_name, name_end)) = read_ident(after, name_start) else { return };
    let Some(set_default_pos) = word_seq_pos_after(words, name_end, &["SET", "DEFAULT"]) else { return };
    let expr_start = set_default_pos + "SET DEFAULT".len();
    let Some(value) = try_read_string_literal(after, expr_start) else { return };
    if let Some(table) = schema.tables.get_mut(&table_sym) {
        if let Some(col) = table.columns.iter_mut().find(|c| c.name.as_str() == col_name) {
            col.default = Some(value);
        }
    }
}

// ---------------------------------------------------------------------
// Low-level scanning: quote/dollar-quote/comment-aware, so a statement
// or column boundary is never mistaken for one that's actually inside
// a string literal or a `$$`-quoted plpgsql function body.
// ---------------------------------------------------------------------

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if starts_with_ci(s, prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

/// Strip a leading run of `-- comment` lines / blank lines / `/* … */`
/// block comments — the banner `pg_dump` writes between every DDL
/// statement (`--\n-- Name: …\n--\n\n`). What's left is the real
/// statement text (still possibly containing embedded comments deeper
/// in, which the quote/comment-aware scanners below handle wherever
/// they walk it).
///
/// The `\restrict <key>` and `\unrestrict <key>` lines that open and
/// close a dump from pg_dump 18 (and 17.6, 16.10, 15.14, 14.19, 13.22)
/// go too. They are psql meta-commands: one line each, with no `;`, so
/// the split glues each onto the statement after it. They change no
/// schema; Rails 7.2.3 and 8.0.3 onward strip them from the dump.
fn strip_leading_comment_banner(raw: &str) -> &str {
    let mut s = raw;
    loop {
        let t = s.trim_start();
        if t.starts_with("\\restrict ") || t.starts_with("\\unrestrict ") {
            match t.find('\n') {
                Some(nl) => {
                    s = &t[nl + 1..];
                    continue;
                }
                None => return "",
            }
        }
        if let Some(after) = t.strip_prefix("--") {
            match after.find('\n') {
                Some(nl) => {
                    s = &after[nl + 1..];
                    continue;
                }
                None => return "",
            }
        }
        if let Some(after) = t.strip_prefix("/*") {
            match after.find("*/") {
                Some(end) => {
                    s = &after[end + 2..];
                    continue;
                }
                None => return "",
            }
        }
        return t;
    }
}

/// If a string literal, quoted identifier, dollar-quoted body, `--`
/// line comment, or `/* … */` block comment starts at byte `i`, return
/// the index just past it. Otherwise `None` — `i` is ordinary code.
/// The one piece of lexing every other function in this module shares,
/// so `;`/`,`/paren/keyword scanning never trips on punctuation that
/// only *looks* like SQL syntax because it's sitting inside a string.
fn skip_quoted_or_comment(bytes: &[u8], i: usize) -> Option<usize> {
    match bytes.get(i) {
        Some(b'-') if bytes.get(i + 1) == Some(&b'-') => {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] != b'\n' {
                j += 1;
            }
            Some(j)
        }
        Some(b'/') if bytes.get(i + 1) == Some(&b'*') => {
            let mut j = i + 2;
            while j + 1 < bytes.len() && !(bytes[j] == b'*' && bytes[j + 1] == b'/') {
                j += 1;
            }
            Some(if j + 1 < bytes.len() { j + 2 } else { bytes.len() })
        }
        Some(b'\'') => {
            let mut j = i + 1;
            loop {
                if j >= bytes.len() {
                    return Some(bytes.len());
                }
                if bytes[j] == b'\'' {
                    if bytes.get(j + 1) == Some(&b'\'') {
                        j += 2;
                        continue;
                    }
                    return Some(j + 1);
                }
                j += 1;
            }
        }
        Some(b'"') => {
            let mut j = i + 1;
            loop {
                if j >= bytes.len() {
                    return Some(bytes.len());
                }
                if bytes[j] == b'"' {
                    if bytes.get(j + 1) == Some(&b'"') {
                        j += 2;
                        continue;
                    }
                    return Some(j + 1);
                }
                j += 1;
            }
        }
        Some(b'$') => {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if bytes.get(j) != Some(&b'$') {
                return None; // a stray `$` (e.g. a placeholder) — ordinary char
            }
            let tag_len = j + 1 - i;
            let tag = &bytes[i..j + 1];
            let mut k = j + 1;
            loop {
                if k + tag_len > bytes.len() {
                    return Some(bytes.len());
                }
                if &bytes[k..k + tag_len] == tag {
                    return Some(k + tag_len);
                }
                k += 1;
            }
        }
        _ => None,
    }
}

/// Split `s` on `delim` at paren-depth 0, skipping quotes/dollar-quotes/
/// comments. Used both for `;`-splitting the whole file into
/// statements and for `,`-splitting a column list — the same
/// tokenizing rules apply either way.
fn top_level_split(s: &str, delim: u8) -> Vec<String> {
    let bytes = s.as_bytes();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    let mut i = 0usize;
    let mut out = Vec::new();
    while i < bytes.len() {
        if let Some(next) = skip_quoted_or_comment(bytes, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                i += 1;
            }
            b if b == delim && depth == 0 => {
                out.push(s[start..i].to_string());
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    out.push(s[start..].to_string());
    out
}

fn find_first_open_paren(s: &str, from: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if let Some(next) = skip_quoted_or_comment(bytes, i) {
            i = next;
            continue;
        }
        if bytes[i] == b'(' {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn matching_close_paren(s: &str, open_idx: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    if bytes.get(open_idx) != Some(&b'(') {
        return None;
    }
    let mut depth = 1i32;
    let mut i = open_idx + 1;
    while i < bytes.len() {
        if let Some(next) = skip_quoted_or_comment(bytes, i) {
            i = next;
            continue;
        }
        match bytes[i] {
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

fn utf8_char_len(b: u8) -> usize {
    if b & 0x80 == 0 {
        1
    } else if b & 0xE0 == 0xC0 {
        2
    } else if b & 0xF0 == 0xE0 {
        3
    } else if b & 0xF8 == 0xF0 {
        4
    } else {
        1
    }
}

/// Read one SQL identifier at byte offset `start` in `s`: a bare
/// word, a `"quoted"` segment (with `""` escaping), or a dotted chain
/// of either (`schema.table`, `"schema"."Table"`) — returning only the
/// rightmost segment (the bare name schema.rb / the rest of this
/// module works with) and the byte offset just past everything
/// consumed. A single quoted segment containing a literal dot (an
/// index name like `"index_daily_log.log_entries_on_company_id"`) is
/// NOT split — the dot only separates segments when it appears
/// between two already-terminated segments, never inside one.
fn read_ident(s: &str, start: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    let mut i = start;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    let mut last_segment = String::new();
    loop {
        if i >= bytes.len() {
            break;
        }
        if bytes[i] == b'"' {
            let mut j = i + 1;
            let mut seg = String::new();
            loop {
                if j >= bytes.len() {
                    break;
                }
                if bytes[j] == b'"' {
                    if bytes.get(j + 1) == Some(&b'"') {
                        seg.push('"');
                        j += 2;
                        continue;
                    }
                    j += 1;
                    break;
                }
                let clen = utf8_char_len(bytes[j]).min(bytes.len() - j);
                seg.push_str(&s[j..j + clen]);
                j += clen;
            }
            last_segment = seg;
            i = j;
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let seg_start = i;
            let mut j = i;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'$') {
                j += 1;
            }
            last_segment = s[seg_start..j].to_string();
            i = j;
        } else {
            break;
        }
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            continue;
        }
        break;
    }
    if last_segment.is_empty() {
        None
    } else {
        Some((last_segment, i))
    }
}

/// Read exactly one PostgreSQL index access method identifier. Unlike
/// [`read_ident`], schema qualifiers are not stripped: `USING public.gin`
/// is not the same identifier as `USING gin` and must not be normalized.
fn read_index_access_method(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() == Some(&b'"') {
        let mut i = 1;
        let mut name = String::new();
        while i < bytes.len() {
            if bytes[i] == b'\0' {
                return None;
            }
            if bytes[i] == b'"' {
                if bytes.get(i + 1) == Some(&b'"') {
                    name.push('"');
                    i += 2;
                    continue;
                }
                i += 1;
                return (!name.is_empty() && name.len() <= 63).then_some((name, i));
            }
            let char_len = utf8_char_len(bytes[i]).min(bytes.len() - i);
            name.push_str(&s[i..i + char_len]);
            i += char_len;
        }
        return None;
    }

    let first = *bytes.first()?;
    if !first.is_ascii_alphabetic() && first != b'_' {
        return None;
    }
    let mut end = 1;
    while end < bytes.len()
        && (bytes[end].is_ascii_alphanumeric() || matches!(bytes[end], b'_' | b'$'))
    {
        end += 1;
    }
    (end <= 63).then_some((s[..end].to_string(), end))
}

/// Skip SQL whitespace and comments but leave quoted identifiers and
/// literals untouched. PostgreSQL accepts comments wherever whitespace
/// can separate tokens, including around `USING` and its method name.
fn skip_sql_trivia(s: &str, start: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = start;
    loop {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes.get(i..i + 2) == Some(&b"--"[..])
            || bytes.get(i..i + 2) == Some(&b"/*"[..])
        {
            i = skip_quoted_or_comment(bytes, i).unwrap_or(bytes.len());
        } else {
            return i;
        }
    }
}

/// Consume one case-insensitive keyword at a known token boundary and
/// return the byte immediately after it. Identifier-continuation bytes
/// (including non-ASCII UTF-8 bytes) keep the text in the same token, so
/// malformed `USINGgin` cannot be mistaken for `USING gin`. Whitespace,
/// SQL comments, and quoted identifiers are valid token separators.
fn consume_sql_keyword_ci(s: &str, start: usize, keyword: &str) -> Option<usize> {
    let end = start.checked_add(keyword.len())?;
    if !s.get(start..end)?.eq_ignore_ascii_case(keyword) {
        return None;
    }
    if let Some(next) = s.as_bytes().get(end) {
        if next.is_ascii_alphanumeric() || *next == b'_' || *next == b'$' || *next >= 0x80 {
            return None;
        }
    }
    Some(end)
}

/// Tokenize `s` into its depth-0 (outside any parens), non-quoted
/// words, each paired with its byte offset in `s`. Every keyword
/// search in this module (`PRIMARY KEY`, `REFERENCES`, `ON DELETE`, a
/// type phrase's `NOT NULL`/`DEFAULT` boundary, …) goes through this
/// so it can never match text that's actually inside a string literal
/// or nested inside a type's own `(precision, scale)`.
fn top_level_words(s: &str) -> Vec<(usize, String)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    let mut depth: i32 = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        if let Some(next) = skip_quoted_or_comment(bytes, i) {
            i = next;
            continue;
        }
        let c = bytes[i];
        if c == b'(' {
            depth += 1;
            i += 1;
            continue;
        }
        if c == b')' {
            depth -= 1;
            i += 1;
            continue;
        }
        if (c as char).is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i;
        while j < bytes.len() {
            if skip_quoted_or_comment(bytes, j).is_some() {
                break;
            }
            let b = bytes[j];
            if (b as char).is_whitespace() || b == b'(' || b == b')' {
                break;
            }
            j += 1;
        }
        if j == start {
            // A quote/comment/dollar-quote starts exactly here — this
            // "word" is zero-width; skip the token itself and resume.
            if let Some(next) = skip_quoted_or_comment(bytes, i) {
                i = next;
                continue;
            }
            i += 1;
            continue;
        }
        if depth == 0 {
            out.push((start, s[start..j].to_ascii_uppercase()));
        }
        i = j;
    }
    out
}

/// Byte offset of the first occurrence, among words positioned after
/// `after_byte`, of the consecutive word sequence `seq` (already
/// uppercased in the search — [`top_level_words`] uppercases every
/// word it returns).
fn word_seq_pos_after(words: &[(usize, String)], after_byte: usize, seq: &[&str]) -> Option<usize> {
    if seq.is_empty() {
        return None;
    }
    for i in 0..words.len() {
        if words[i].0 <= after_byte {
            continue;
        }
        if i + seq.len() > words.len() {
            return None;
        }
        if (0..seq.len()).all(|k| words[i + k].1 == seq[k]) {
            return Some(words[i].0);
        }
    }
    None
}

/// Where a column definition's type phrase ends and its modifiers
/// (`DEFAULT …`, `NOT NULL`, `NULL`, `GENERATED …`, `COLLATE …`, an
/// inline `PRIMARY KEY`) begin — the first depth-0 occurrence of one
/// of those keywords, or the end of the string when the column has no
/// modifiers at all.
fn find_modifier_start(rest: &str) -> usize {
    const STOP: [&str; 6] = ["DEFAULT", "NOT", "NULL", "GENERATED", "COLLATE", "PRIMARY"];
    for (pos, word) in top_level_words(rest) {
        if STOP.contains(&word.as_str()) {
            return pos;
        }
    }
    rest.len()
}

/// Parse the supported PostgreSQL generated-column modifier. Any other
/// form containing `GENERATED` fails explicitly so it cannot become an
/// ordinary writable column after its expression is discarded.
fn parse_generated_modifier(modifiers: &str) -> Result<Option<GeneratedColumn>, String> {
    let words = top_level_words(modifiers);
    let generated_positions: Vec<usize> =
        words.iter().filter(|(_, word)| word == "GENERATED").map(|(pos, _)| *pos).collect();
    let Some(&generated_pos) = generated_positions.first() else { return Ok(None) };
    if generated_positions.len() != 1 {
        return Err("multiple GENERATED clauses are unsupported".into());
    }

    if !only_optional_nullability(&modifiers[..generated_pos]) {
        return Err("only NULL or NOT NULL may precede GENERATED ALWAYS".into());
    }

    let after_generated = consume_sql_keyword_ci(modifiers, generated_pos, "GENERATED")
        .ok_or_else(|| "expected GENERATED ALWAYS AS".to_string())?;
    let after_always = consume_sql_keyword_ci(
        modifiers,
        skip_sql_trivia(modifiers, after_generated),
        "ALWAYS",
    )
    .ok_or_else(|| "expected GENERATED ALWAYS AS".to_string())?;
    let after_as = consume_sql_keyword_ci(
        modifiers,
        skip_sql_trivia(modifiers, after_always),
        "AS",
    )
    .ok_or_else(|| "expected GENERATED ALWAYS AS".to_string())?;
    let open = skip_sql_trivia(modifiers, after_as);
    if modifiers.as_bytes().get(open) != Some(&b'(') {
        return Err("generated expression must be parenthesized".into());
    }
    let close = matching_close_paren(modifiers, open)
        .ok_or_else(|| "generated expression has an unclosed parenthesis".to_string())?;
    let expression = modifiers[open + 1..close].to_string();
    if expression.trim().is_empty() {
        return Err("generated expression is empty".into());
    }

    let storage_start = skip_sql_trivia(modifiers, close + 1);
    let (storage, storage_end) = if let Some(end) = consume_sql_keyword_ci(modifiers, storage_start, "STORED") {
        (GeneratedColumnStorage::Stored, end)
    } else if let Some(end) = consume_sql_keyword_ci(modifiers, storage_start, "VIRTUAL") {
        (GeneratedColumnStorage::Virtual, end)
    } else {
        return Err("generated expression must end in STORED or VIRTUAL".into());
    };

    if !only_optional_nullability(&modifiers[storage_end..]) {
        return Err("only NULL or NOT NULL may follow the generated storage mode".into());
    }

    Ok(Some(GeneratedColumn { expression, storage }))
}

/// Consume only an optional `NULL` or `NOT NULL` clause plus SQL trivia.
/// Checking the full suffix prevents punctuation, string literals, or
/// other modifiers from being silently ignored.
fn only_optional_nullability(s: &str) -> bool {
    let start = skip_sql_trivia(s, 0);
    if start == s.len() {
        return true;
    }
    if let Some(end) = consume_sql_keyword_ci(s, start, "NULL") {
        return skip_sql_trivia(s, end) == s.len();
    }
    let Some(after_not) = consume_sql_keyword_ci(s, start, "NOT") else { return false };
    let Some(after_null) = consume_sql_keyword_ci(s, skip_sql_trivia(s, after_not), "NULL") else {
        return false;
    };
    skip_sql_trivia(s, after_null) == s.len()
}

/// From a column definition's modifier tail: whether the column is
/// nullable (`NOT NULL` seen → false; otherwise the schema.rb-ingest
/// default of `true`) and its literal string default, if any — parity
/// with `ingest_schema`'s own `parse_column_opts`, which likewise only
/// captures a `DEFAULT` when it's a quoted string (never a bare
/// number/boolean, and never an expression like `nextval(…)`).
fn parse_modifiers(modifiers: &str) -> (bool, Option<String>) {
    let words = top_level_words(modifiers);
    let mut nullable = true;
    let mut default = None;
    for idx in 0..words.len() {
        let (pos, word) = &words[idx];
        if word == "NOT" && words.get(idx + 1).map(|(_, w)| w.as_str()) == Some("NULL") {
            nullable = false;
        }
        if word == "DEFAULT" {
            let after = pos + "DEFAULT".len();
            if let Some(v) = try_read_string_literal(modifiers, after) {
                default = Some(v);
            }
        }
    }
    (nullable, default)
}

/// If a `'single-quoted string'` (with `''` escaping) starts at or
/// after byte `from` — skipping only whitespace to find it — return
/// its unescaped content. Used for `DEFAULT 'draft'::character
/// varying` (skips the value, ignores the trailing `::type` cast) and
/// for an `ALTER COLUMN … SET DEFAULT '…'` expression.
fn try_read_string_literal(s: &str, from: usize) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }
    if bytes.get(i) != Some(&b'\'') {
        return None;
    }
    let mut out = String::new();
    let mut j = i + 1;
    loop {
        if j >= bytes.len() {
            return Some(out);
        }
        if bytes[j] == b'\'' {
            if bytes.get(j + 1) == Some(&b'\'') {
                out.push('\'');
                j += 2;
                continue;
            }
            break;
        }
        let clen = utf8_char_len(bytes[j]).min(bytes.len() - j);
        out.push_str(&s[j..j + clen]);
        j += clen;
    }
    Some(out)
}

/// Strip every `(...)` group from `phrase`, lowercase and
/// whitespace-collapse what remains (`timestamp(6) without time zone`
/// → `"timestamp without time zone"`), and return the first two
/// comma-separated integers found inside any stripped group (`numeric
/// (20,2)` → `Some(20), Some(2)`; `character varying(255)` →
/// `Some(255), None`). Byte-oriented and ASCII-only — safe because a
/// Postgres type phrase never contains non-ASCII text.
fn strip_parens_capture_nums(phrase: &str) -> (String, Option<i64>, Option<i64>) {
    let bytes = phrase.as_bytes();
    let mut base = String::new();
    let mut nums: Vec<i64> = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            let mut j = i + 1;
            let mut inner = String::new();
            while j < bytes.len() && bytes[j] != b')' {
                inner.push(bytes[j] as char);
                j += 1;
            }
            for part in inner.split(',') {
                if let Ok(n) = part.trim().parse::<i64>() {
                    nums.push(n);
                }
            }
            i = if j < bytes.len() { j + 1 } else { j };
        } else {
            base.push(bytes[i] as char);
            i += 1;
        }
    }
    let base = base.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase();
    (base, nums.first().copied(), nums.get(1).copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_named_like_a_constraint_keyword_is_a_column() {
        assert!(!is_table_level_constraint("checkout_id bigint"));
        assert!(!is_table_level_constraint("unique_id character varying(255) NOT NULL"));
        assert!(!is_table_level_constraint("excluded_flag boolean"));
        assert!(!is_table_level_constraint("constraints jsonb"));
        assert!(is_table_level_constraint("CHECK ((amount > 0))"));
        assert!(is_table_level_constraint("CHECK(amount > 0)"));
        assert!(is_table_level_constraint("UNIQUE (shop_id, code)"));
        assert!(is_table_level_constraint("CONSTRAINT x_pkey PRIMARY KEY (id)"));
    }

    #[test]
    fn top_level_split_respects_dollar_quoted_function_body() {
        let sql = "CREATE FUNCTION f() LANGUAGE plpgsql AS $$ BEGIN INSERT INTO x; END; $$; SELECT 1;";
        let parts = top_level_split(sql, b';');
        // The `;`s inside the $$...$$ body must not fragment the
        // CREATE FUNCTION statement.
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert!(parts[0].contains("BEGIN INSERT INTO x; END;"), "{:?}", parts[0]);
    }

    #[test]
    fn top_level_split_respects_string_literals() {
        let sql = "INSERT INTO t VALUES ('a;b'); SELECT 1;";
        let parts = top_level_split(sql, b';');
        // A trailing empty segment after the final `;` is expected —
        // `ingest_structure_sql`'s caller loop skips empty/whitespace
        // statements rather than `top_level_split` filtering them.
        assert_eq!(parts.len(), 3, "{parts:?}");
        assert!(parts[0].contains("'a;b'"));
        assert!(parts[2].trim().is_empty());
    }

    #[test]
    fn read_ident_strips_schema_qualifier() {
        let (name, consumed) = read_ident("public.widgets more text", 0).unwrap();
        assert_eq!(name, "widgets");
        assert_eq!(&"public.widgets more text"[consumed..], " more text");
    }

    #[test]
    fn read_ident_keeps_dot_inside_single_quoted_segment() {
        let (name, _) = read_ident(r#""index_a.b_on_c" ON"#, 0).unwrap();
        assert_eq!(name, "index_a.b_on_c");
    }

    #[test]
    fn structure_sql_index_methods_are_single_identifiers() {
        let quoted = r#""Odd""Method""#;
        let (name, consumed) = read_index_access_method(quoted).unwrap();
        assert_eq!(name, "Odd\"Method");
        assert_eq!(consumed, quoted.len());

        let qualified = "public.gin";
        let (_, consumed) = read_index_access_method(qualified).unwrap();
        assert!(!qualified[consumed..].is_empty(), "qualifier must not be discarded");
        assert!(read_index_access_method("\"gin").is_none(), "unterminated quote");
        assert!(read_index_access_method("\"\"").is_none(), "empty quoted name");
    }

    #[test]
    fn structure_sql_rejects_a_schema_qualified_index_method() {
        let sql = br#"CREATE TABLE widgets (payload jsonb);
CREATE INDEX widgets_payload_idx ON widgets USING public.gin (payload);"#;
        let error = ingest_structure_sql(sql, "db/structure.sql").unwrap_err();
        assert!(error.to_string().contains("CREATE INDEX access method is not a SQL identifier"), "{error}");
    }

    #[test]
    fn strip_parens_capture_nums_extracts_precision_and_scale() {
        let (base, n1, n2) = strip_parens_capture_nums("numeric(20,2)");
        assert_eq!(base, "numeric");
        assert_eq!(n1, Some(20));
        assert_eq!(n2, Some(2));
    }

    #[test]
    fn strip_parens_capture_nums_handles_mid_phrase_group() {
        let (base, n1, n2) = strip_parens_capture_nums("timestamp(6) without time zone");
        assert_eq!(base, "timestamp without time zone");
        assert_eq!(n1, Some(6));
        assert_eq!(n2, None);
    }

    #[test]
    fn resolve_column_type_maps_the_core_postgres_types() {
        let enums = HashSet::new();
        assert!(matches!(
            resolve_column_type("character varying(255)", &enums),
            Some(ColumnType::String { limit: Some(255) })
        ));
        assert!(matches!(resolve_column_type("bigint", &enums), Some(ColumnType::BigInt)));
        assert!(matches!(resolve_column_type("json", &enums), Some(ColumnType::Json)));
        assert!(matches!(resolve_column_type("jsonb", &enums), Some(ColumnType::Jsonb)));
        assert!(matches!(resolve_column_type("uuid", &enums), Some(ColumnType::Uuid)));
        assert!(matches!(
            resolve_column_type("numeric(10,2)", &enums),
            Some(ColumnType::Decimal { precision: None, scale: None })
        ));
        assert!(resolve_column_type("tstzrange", &enums).is_none());
        assert!(resolve_column_type("bigint[]", &enums).is_none());
    }

    #[test]
    fn resolve_column_type_maps_a_registered_enum_to_string() {
        let mut enums = HashSet::new();
        enums.insert("widget_status".to_string());
        assert!(matches!(
            resolve_column_type("public.widget_status", &enums),
            Some(ColumnType::String { limit: None })
        ));
    }
}
