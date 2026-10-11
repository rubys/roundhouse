//! `db/schema.rb` — parse the Rails schema DSL (`ActiveRecord::Schema`
//! plus a sequence of `create_table`s with column calls) into a
//! target-neutral `Schema`. Rails' implicit bigint `id` primary key is
//! synthesized here unless `id: false` is passed to `create_table`.
//!
//! When `db/schema.rb` is absent (never migrated locally, gitignored,
//! or a juntos-style app that only ships migrations), the same column
//! facts can usually be recovered by folding `db/migrate/*.rb` in
//! timestamp order — [`ingest_migration`] handles one file of that
//! fold. Both paths share the `create_table` recognizer: schema.rb is
//! itself a sequence of `create_table` calls, just with string names
//! where migrations use symbols and with `t.timestamps` already
//! materialized into explicit datetime columns.

use ruby_prism::Node;

use indexmap::IndexMap;

use crate::schema::generated::GeneratedExpressionDialect;
use crate::schema::{Column, ColumnType, GeneratedColumn, GeneratedColumnStorage, Index, Schema, Table};
use crate::{Symbol, TableRef};

use super::util::{
    bool_value, constant_id_str, find_first_class, flatten_statements, integer_value,
    string_value, symbol_value, walk_calls,
};
use super::{IngestError, IngestResult};

/// Ingest a Rails schema with the Portable generated-expression grammar.
/// PostgreSQL-only casts require the explicit DDL-dialect entry point.
pub fn ingest_schema(source: &[u8], file: &str) -> IngestResult<Schema> {
    ingest_schema_with_generated_expression_dialect(
        source,
        file,
        GeneratedExpressionDialect::Portable,
    )
}

/// Ingest a Rails schema while validating generated expressions for an
/// explicit DDL source dialect. Application ingestion continues to call
/// [`ingest_schema`] and therefore retains the Portable default.
pub fn ingest_schema_with_generated_expression_dialect(
    source: &[u8],
    file: &str,
    dialect: GeneratedExpressionDialect,
) -> IngestResult<Schema> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();

    let mut schema = Schema::default();
    // Columns the walk could not model. A dropped column used to be
    // silent, and its index was still emitted — so the DDL failed
    // with `no such column` on the first uuid-keyed app (#83). Survey
    // runs ledger every one; a strict run fails on the first.
    let mut gaps: Vec<IngestError> = Vec::new();
    walk_calls(&root, &mut |call| {
        match constant_id_str(&call.name()) {
            "create_table" => {
                if let Some((name, table)) =
                    table_from_create_table(call, file, &mut gaps, dialect)
                {
                    schema.tables.insert(name, table);
                }
            }
            // `create_view "name", sql_definition: <<~SQL …` (the
            // scenic gem / Rails 6 schema dumper). A SQL view backs a
            // model just like a table, so register its columns —
            // extracted from the SELECT's `AS <alias>` list — as a
            // Table. ReplyingComment is the lobsters case.
            // `create_virtual_table "message_search_index", "fts5",
            // ["body", "tokenize=porter"]` — campfire's full-text
            // search index. It is a real table the app writes to
            // (`Message::Searchable` keeps it in step through
            // after_*_commit callbacks), so it has to reach the DDL;
            // unregistered, every message insert died on "no such
            // table".
            "create_virtual_table" => {
                if let Some((name, table)) = virtual_table_from_call(call) {
                    schema.tables.insert(name, table);
                }
            }
            "create_view" => {
                // `create_table`s are dumped before views, so the
                // tables a view projects are already in `schema.tables`
                // — pass them so direct column projections get their
                // real types instead of a name guess.
                if let Some((name, table)) = view_from_create_view(call, &schema.tables) {
                    schema.tables.insert(name, table);
                }
            }
            // Dumped after every create_table, so the target tables are
            // in place by the time these fold in.
            "add_foreign_key" => {
                let args: Vec<Node<'_>> = call
                    .arguments()
                    .map(|a| a.arguments().iter().collect())
                    .unwrap_or_default();
                apply_add_foreign_key(&args, &mut schema);
            }
            _ => {}
        }
    });

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

/// Fold one `db/migrate/*.rb` file into `schema` — the fallback schema
/// source when `db/schema.rb` is absent. The caller iterates files in
/// filename order (timestamp prefixes sort chronologically).
///
/// Only the migration's `change` method is replayed (`up` when no
/// `change` exists; `down` is never touched). Schema-mutating verbs we
/// can't fold deterministically (`change_table`, `execute`, raw-SQL
/// shapes — see `UNSUPPORTED_VERBS`, or a verb whose table or column
/// name is not a literal) error with a pointer to
/// `rails db:migrate`, which materializes the schema.rb this fallback
/// substitutes for. Receiver-less calls that aren't recognized verbs
/// are ignored: migrations legitimately contain arbitrary Ruby (data
/// backfills, `say`, …) that doesn't affect the schema.
pub fn ingest_migration(source: &[u8], file: &str, schema: &mut Schema) -> IngestResult<()> {
    ingest_migration_with_generated_expression_dialect(
        source,
        file,
        schema,
        GeneratedExpressionDialect::Portable,
    )
}

/// Fold one migration using an explicit generated-expression grammar.
/// The default [`ingest_migration`] remains Portable for application ingest.
pub fn ingest_migration_with_generated_expression_dialect(
    source: &[u8],
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> IngestResult<()> {
    super::sources::register(file, &String::from_utf8_lossy(source));
    let result = super::prism::parse(source, file);
    let root = result.node();
    let Some(class) = find_first_class(&root) else {
        return Ok(());
    };

    let mut change_body: Option<Node<'_>> = None;
    let mut up_body: Option<Node<'_>> = None;
    if let Some(body) = class.body() {
        for stmt in flatten_statements(body) {
            if let Some(def) = stmt.as_def_node() {
                let name = constant_id_str(&def.name()).to_string();
                match name.as_str() {
                    "change" => change_body = def.body(),
                    "up" => up_body = def.body(),
                    _ => {}
                }
            }
        }
    }
    let Some(body) = change_body.or(up_body) else {
        return Ok(());
    };

    let mut err: Option<IngestError> = None;
    walk_calls(&body, &mut |call| {
        // Top-level migration verbs are receiver-less; receiver-bearing
        // calls are either `t.<column>` (handled inside create_table)
        // or app code in a backfill (not schema-affecting).
        if err.is_some() || call.receiver().is_some() {
            return;
        }
        let verb = constant_id_str(&call.name()).to_string();
        if let Err(e) = apply_migration_verb(&verb, call, file, schema, dialect) {
            err = Some(e);
        }
    });
    match err {
        Some(e) => Err(e),
        None => {
            for table in schema.tables.values() {
                if let Some((column, reason)) = crate::schema::generated::validate_table_with_dialect(table, dialect).into_iter().next() {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!("generated column dropped: {}.{column}: {reason}", table.name),
                    });
                }
            }
            Ok(())
        }
    }
}

/// Verbs that mutate schema in ways the fold doesn't model. Erroring
/// (rather than skipping) keeps the derived schema honest — a silently
/// missed `change_table` would surface later as baffling type errors.
const UNSUPPORTED_VERBS: &[&str] = &[
    "change_table",
    "create_join_table",
    "drop_join_table",
    "execute",
    "reversible",
    "revert",
    "up_only",
];

/// Apply one recognized schema-changing migration verb under the selected expression dialect.
fn apply_migration_verb(
    verb: &str,
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    schema: &mut Schema,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    if UNSUPPORTED_VERBS.contains(&verb) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "migration verb `{verb}` not supported by the schema fold — run \
                 `rails db:migrate` to materialize db/schema.rb"
            ),
        });
    }

    let args: Vec<Node<'_>> = call
        .arguments()
        .map(|a| a.arguments().iter().collect())
        .unwrap_or_default();
    let arg_name = |i: usize| args.get(i).and_then(table_name_value);
    // A verb whose table or column name is not a literal cannot apply,
    // and skipping it left the column typed: a
    // `remove_column(:idx, :"col_#{n}")` in an `each` loop
    // kept the dropped columns.
    let named = match verb {
        "drop_table" | "add_timestamps" => 1,
        "rename_table" | "remove_column" | "change_column_null" | "change_column_default"
        | "add_reference" | "add_belongs_to" | "remove_reference" | "remove_belongs_to" => 2,
        "add_column" | "change_column" | "rename_column" => 3,
        _ => 0,
    };
    if let Some(i) = (0..named).find(|&i| arg_name(i).is_none()) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "migration `{verb}` argument {} is not a literal name; the schema fold cannot apply it",
                i + 1
            ),
        });
    }

    match verb {
        "create_table" => {
            let mut gaps: Vec<IngestError> = Vec::new();
            let built = table_from_create_table(call, file, &mut gaps, dialect);
            if let Some(gap) = gaps.into_iter().next() {
                return Err(gap);
            }
            if let Some((name, table)) = built {
                schema.tables.insert(name, table);
            }
        }
        "drop_table" => {
            if let Some(name) = arg_name(0) {
                schema.tables.shift_remove(&Symbol::from(name));
            }
        }
        "rename_table" => {
            if let (Some(old), Some(new)) = (arg_name(0), arg_name(1)) {
                if let Some(mut table) = schema.tables.shift_remove(&Symbol::from(old)) {
                    table.name = Symbol::from(new.clone());
                    schema.tables.insert(Symbol::from(new), table);
                }
            }
        }
        "add_column" | "change_column" => {
            // Check an existing generated target before interpreting the
            // requested type/options. Even an unknown replacement type
            // must not bypass the explicit generated-column boundary.
            if verb == "change_column" {
                if let (Some(t), Some(c)) = (arg_name(0), arg_name(1)) {
                    let targets_generated = schema
                        .tables
                        .get(&Symbol::from(t.as_str()))
                        .is_some_and(|table| {
                            table.columns.iter().any(|column| {
                                column.name.as_str() == c.as_str()
                                    && column.generated.is_some()
                            })
                        });
                    if targets_generated {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "generated column changes are not supported: {t}.{c}"
                            ),
                        });
                    }
                }
            }
            if let (Some(t), Some(c), Some(ty)) = (arg_name(0), arg_name(1), arg_name(2)) {
                let option_args = &args[3..];
                if has_column_option_splats(option_args) {
                    return Err(IngestError::Unsupported {
                        file: file.into(),
                        message: format!(
                            "column options for {t}.{c} cannot use keyword splats because generated-column metadata may be hidden"
                        ),
                    });
                }
                let generated_option = has_generated_column_options(option_args);
                let generated = ty == "virtual" || generated_option;
                let col = if generated {
                    if verb == "change_column" {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!("generated column changes are not supported: {t}.{c}"),
                        });
                    }
                    generated_column_from_options(
                        c,
                        &t,
                        file,
                        if ty == "virtual" { None } else { Some(&ty) },
                        option_args,
                    )?
                } else {
                    let opts = parse_column_opts(option_args.iter());
                    column_with_type(&ty, c, &opts, &t, file)?
                };
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    // This fold replaces a same-named column for both
                    // verbs. Do not let an ordinary add/change erase the
                    // generated expression and make the field writable.
                    if col.generated.is_none()
                        && table.columns.iter().any(|existing| {
                            existing.name == col.name && existing.generated.is_some()
                        })
                    {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "generated column `{}.{}` cannot be replaced by an ordinary column with `{verb}`",
                                t,
                                col.name.as_str(),
                            ),
                        });
                    }
                    // Validate the replacement against the complete
                    // candidate before changing the folded schema. This
                    // keeps failed generated replacements (and ordinary
                    // replacements that invalidate generated operands)
                    // from erasing the previous metadata in survey mode.
                    let mut candidate = table.clone();
                    candidate.columns.retain(|existing| existing.name != col.name);
                    candidate.columns.push(col.clone());
                    if table.columns.iter().any(|column| column.generated.is_some())
                        || col.generated.is_some()
                    {
                        validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                    }
                }
                // change_column replaces; add_column after a
                // replace-shaped history stays idempotent.
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    table.columns.retain(|x| x.name != col.name);
                    table.columns.push(col);
                }
            }
        }
        "remove_column" => {
            if let (Some(t), Some(c)) = (arg_name(0), arg_name(1)) {
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    refuse_predicate_column(verb, table, &c, file)?;
                    refuse_indexed_generated_column_mutation(verb, table, &c, file)?;
                    if table.columns.iter().any(|column| column.generated.is_some()) {
                        let mut candidate = table.clone();
                        candidate.columns.retain(|column| column.name.as_str() != c);
                        validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                    }
                }
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    table.columns.retain(|x| x.name.as_str() != c);
                }
            }
        }
        "rename_column" => {
            if let (Some(t), Some(old), Some(new)) = (arg_name(0), arg_name(1), arg_name(2)) {
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    refuse_predicate_column(verb, table, &old, file)?;
                    if old != new {
                        refuse_indexed_generated_column_mutation(verb, table, &old, file)?;
                        if table.columns.iter().any(|column| column.generated.is_some()) {
                            let mut candidate = table.clone();
                            for column in &mut candidate.columns {
                                if column.name.as_str() == old {
                                    column.name = Symbol::from(new.clone());
                                }
                            }
                            validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                        }
                    }
                }
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    for col in &mut table.columns {
                        if col.name.as_str() == old {
                            col.name = Symbol::from(new.clone());
                        }
                    }
                }
            }
        }
        "change_column_null" => {
            // change_column_null :table, :col, <allow-null bool>
            if let (Some(t), Some(c), Some(allow)) =
                (arg_name(0), arg_name(1), args.get(2).and_then(bool_value))
            {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    for col in &mut table.columns {
                        if col.name.as_str() == c {
                            col.nullable = allow;
                        }
                    }
                }
            }
        }
        "change_column_default" => {
            // Positional literal or `from:`/`to:` kwargs; only literals
            // are retained (parity with the schema.rb parser).
            if let (Some(t), Some(c)) = (arg_name(0), arg_name(1)) {
                let positional = args.get(2).and_then(default_value);
                let to_kwarg = kwarg_value(args.iter().skip(2), "to").and_then(|v| default_value(&v));
                let default = to_kwarg.clone().or_else(|| positional.clone());
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    if table.columns.iter().any(|column| column.generated.is_some()) {
                        let mut candidate = table.clone();
                        for col in &mut candidate.columns {
                            if col.name.as_str() == c {
                                col.default = default.clone();
                            }
                        }
                        validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                    }
                }
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    for col in &mut table.columns {
                        if col.name.as_str() == c {
                            col.default = default.clone();
                        }
                    }
                }
            }
        }
        "add_timestamps" => {
            if let Some(t) = arg_name(0) {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    table.columns.extend(timestamp_columns());
                }
            }
        }
        "add_reference" | "add_belongs_to" => {
            if let (Some(t), Some(name)) = (arg_name(0), arg_name(1)) {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    table.columns.push(reference_column(&name));
                }
            }
        }
        "remove_reference" | "remove_belongs_to" => {
            if let (Some(t), Some(name)) = (arg_name(0), arg_name(1)) {
                if let Some(table) = schema.tables.get(&Symbol::from(t.as_str())) {
                    if table.columns.iter().any(|column| {
                        column.name.as_str() == name && column.generated.is_some()
                    }) {
                        return Err(IngestError::Unsupported {
                            file: file.into(),
                            message: format!(
                                "generated column cannot be removed with `{verb}`: {}.{name} is not a reference",
                                table.name.as_str()
                            ),
                        });
                    }
                    if table.columns.iter().any(|column| column.generated.is_some()) {
                        let mut candidate = table.clone();
                        candidate
                            .columns
                            .retain(|column| column.name.as_str() != name);
                        validate_generated_migration_candidate(&candidate, verb, file, dialect)?;
                    }
                }
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t.as_str())) {
                    table.columns.retain(|x| x.name.as_str() != name);
                }
            }
        }
        "add_index" => {
            if let (Some(t), Some(columns)) = (arg_name(0), args.get(1).map(column_name_list)) {
                if !columns.is_empty() {
                    if let Some(table) = schema.tables.get_mut(&Symbol::from(t.clone())) {
                        if columns.iter().all(|c| table.columns.iter().any(|col| col.name == *c)) {
                            table.indexes.push(build_index(&t, columns, args.iter().skip(2), file)?);
                        }
                    }
                }
            }
        }
        "remove_index" => {
            if let Some(t) = arg_name(0) {
                let columns = args.get(1).map(column_name_list).unwrap_or_default();
                let by_name =
                    kwarg_value(args.iter().skip(1), "name").and_then(|v| string_value(&v));
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    table.indexes.retain(|idx| {
                        let col_match = !columns.is_empty() && idx.columns == columns;
                        let name_match =
                            by_name.as_deref().is_some_and(|n| idx.name.as_str() == n);
                        !(col_match || name_match)
                    });
                }
            }
        }
        // `add_foreign_key "comments", "articles"[, column:, primary_key:,
        // on_delete:, on_update:]` — recorded into `Table.foreign_keys` so
        // downstream consumers see the referential shape (the Roda/Sequel
        // conversion emits real `foreign_key` migration lines from it).
        // Column typing itself is unaffected: the FK column is already an
        // ordinary integer column from `create_table`.
        "add_foreign_key" => apply_add_foreign_key(&args, schema),
        // Extensions (and FK removal — schema.rb is canonical state, so
        // a remove would only appear in migration folds where the add is
        // also seen) don't affect column typing; skipped like before.
        "remove_foreign_key" | "enable_extension" | "disable_extension" => {}
        // Anything else receiver-less is arbitrary migration Ruby
        // (`say`, backfill helpers) — not schema-affecting.
        _ => {}
    }
    Ok(())
}

/// A column a partial index's predicate names can't be renamed or
/// removed by the fold. The database rewrites the predicate, or drops
/// the index, and the fold has only the predicate's text, so it would
/// render a `WHERE` naming a column that is gone. Erroring keeps the
/// derived schema honest, like `UNSUPPORTED_VERBS`.
fn refuse_predicate_column(
    verb: &str,
    table: &Table,
    column: &str,
    file: &str,
) -> Result<(), IngestError> {
    let Some(index) = table
        .indexes
        .iter()
        .find(|i| i.predicate.as_deref().is_some_and(|p| predicate_names(p, column)))
    else {
        return Ok(());
    };
    Err(IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "migration verb `{verb}` on {}.{column}, which the `where:` of index `{}` names, \
             is not supported by the schema fold — run `rails db:migrate` to materialize \
             db/schema.rb",
            table.name.as_str(),
            index.name.as_str()
        ),
    })
}

/// Renaming or removing an indexed generated output would leave its index
/// referring to a column name the fold no longer knows. Ordinary index
/// mutation remains governed by the existing migration-fold behavior.
fn refuse_indexed_generated_column_mutation(
    verb: &str,
    table: &Table,
    column: &str,
    file: &str,
) -> Result<(), IngestError> {
    let is_generated = table.columns.iter().any(|existing| {
        existing.name.as_str() == column && existing.generated.is_some()
    });
    if !is_generated {
        return Ok(());
    }
    let Some(index) = table
        .indexes
        .iter()
        .find(|index| index.columns.iter().any(|indexed| indexed.as_str() == column))
    else {
        return Ok(());
    };
    let action = match verb {
        "rename_column" => "renamed",
        "remove_column" => "removed",
        _ => "mutated",
    };
    Err(IngestError::Unsupported {
        file: file.into(),
        message: format!(
            "generated column index mutation is unsupported: {}.{column} is used by index `{}` and cannot be {action} without rewriting the index",
            table.name.as_str(),
            index.name.as_str()
        ),
    })
}

/// Keep migration-derived schema state unchanged when a candidate
/// mutation breaks a generated expression or another generated-table
/// invariant. Expressions remain source SQL; renames never rewrite them.
fn validate_generated_migration_candidate(
    table: &Table,
    verb: &str,
    file: &str,
    dialect: GeneratedExpressionDialect,
) -> Result<(), IngestError> {
    if let Some((column, reason)) = crate::schema::generated::validate_table_with_dialect(table, dialect)
        .into_iter()
        .next()
    {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "generated column mutation is unsupported: {verb} on {}.{column}: {reason}",
                table.name.as_str()
            ),
        });
    }
    Ok(())
}

/// Whether `predicate` names `column`: outside its string literals, a
/// bare word equal to it (ignoring case, as Postgres folds an unquoted
/// name) or a double-quoted identifier spelling it exactly, `""` being
/// a quote inside one.
fn predicate_names(predicate: &str, column: &str) -> bool {
    let chars: Vec<char> = predicate.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\'' => {
                i += 1;
                while i < chars.len() {
                    if chars[i] == '\'' && chars.get(i + 1) == Some(&'\'') {
                        i += 2;
                    } else if chars[i] == '\'' {
                        break;
                    } else {
                        i += 1;
                    }
                }
                i += 1;
            }
            '"' => {
                let mut name = String::new();
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' && chars.get(i + 1) == Some(&'"') {
                        name.push('"');
                        i += 2;
                    } else if chars[i] == '"' {
                        break;
                    } else {
                        name.push(chars[i]);
                        i += 1;
                    }
                }
                i += 1;
                if name == column {
                    return true;
                }
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if word.eq_ignore_ascii_case(column) {
                    return true;
                }
            }
            _ => i += 1,
        }
    }
    false
}

/// schema.rb writes string literals (`create_table "clips"`); hand-
/// written migrations write symbols (`create_table :clips`). Accept
/// both anywhere a table/column name is read.
fn name_value(node: &Node<'_>) -> Option<String> {
    string_value(node).or_else(|| symbol_value(node))
}

/// A table name as the Postgres schema dumper writes it when the
/// search path has more than one schema (`"public.companies"`): the
/// qualifier is not part of the table's name anywhere else in the app.
fn table_name_value(node: &Node<'_>) -> Option<String> {
    name_value(node).map(|s| match s.rsplit_once('.') {
        Some((_, bare)) => bare.to_string(),
        None => s,
    })
}

/// `add_foreign_key "comments", "articles"[, column:, primary_key:,
/// on_delete:, on_update:]` — recorded into `Table.foreign_keys` so
/// downstream consumers see the referential shape (the Roda/Sequel
/// conversion emits real `foreign_key` migration lines from it).
/// Shared by the schema.rb walk and the migration fold; column typing
/// itself is unaffected (the FK column is already an ordinary integer
/// column from `create_table`).
fn apply_add_foreign_key(args: &[Node<'_>], schema: &mut Schema) {
    let arg_name = |i: usize| args.get(i).and_then(table_name_value);
    let (Some(from_t), Some(to_t)) = (arg_name(0), arg_name(1)) else { return };
    let kw = |key: &str| kwarg_value(args.iter().skip(2), key).and_then(|v| name_value(&v));
    let from_column =
        kw("column").unwrap_or_else(|| format!("{}_id", crate::naming::singularize(&to_t)));
    let to_column = kw("primary_key").unwrap_or_else(|| "id".to_string());
    let on_delete = kw("on_delete").map(|s| referential_action(&s)).unwrap_or_default();
    let on_update = kw("on_update").map(|s| referential_action(&s)).unwrap_or_default();
    if let Some(table) = schema.tables.get_mut(&Symbol::from(from_t)) {
        table.foreign_keys.push(crate::schema::ForeignKey {
            from_column: Symbol::from(from_column),
            to_table: TableRef(Symbol::from(to_t)),
            to_column: Symbol::from(to_column),
            on_delete,
            on_update,
        });
    }
}

/// `on_delete:`/`on_update:` symbol → the neutral referential action.
/// Rails' spelling for SET NULL is `:nullify`.
fn referential_action(sym: &str) -> crate::schema::ReferentialAction {
    use crate::schema::ReferentialAction::*;
    match sym {
        "cascade" => Cascade,
        "nullify" => SetNull,
        "restrict" => Restrict,
        _ => NoAction,
    }
}

/// First kwarg with key `key` among `nodes` (each a KeywordHashNode or
/// positional to skip).
fn kwarg_value<'pr>(
    nodes: impl Iterator<Item = &'pr Node<'pr>>,
    key: &str,
) -> Option<Node<'pr>> {
    for node in nodes {
        let Some(kh) = node.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(k) = symbol_value(&assoc.key()) else { continue };
            if k.as_str() == key {
                return Some(assoc.value());
            }
        }
    }
    None
}

/// One column-name positional: a bare name or an array of names.
fn column_name_list(node: &Node<'_>) -> Vec<Symbol> {
    if let Some(arr) = node.as_array_node() {
        arr.elements()
            .iter()
            .filter_map(|el| name_value(&el))
            .map(Symbol::from)
            .collect()
    } else {
        name_value(node).map(Symbol::from).into_iter().collect()
    }
}

fn build_index<'pr>(
    table_name: &str,
    columns: Vec<Symbol>,
    kwarg_nodes: impl Iterator<Item = &'pr Node<'pr>>,
    file: &str,
) -> IngestResult<Index> {
    let mut explicit_name: Option<String> = None;
    let mut unique = false;
    let mut using: Option<String> = None;
    let mut predicate: Option<String> = None;
    for node in kwarg_nodes {
        let Some(kh) = node.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = &assoc.value();
            match key.as_str() {
                "name" => explicit_name = string_value(value),
                "unique" => unique = bool_value(value).unwrap_or(false),
                // `where: "(archived_at IS NULL)"`: a partial index. On
                // a unique one, dropping it widened the constraint to
                // every row.
                "where" => predicate = string_value(value),
                "using" => using = Some(index_access_method(value, file, table_name)?),
                _ => {}
            }
        }
    }
    let name = explicit_name.unwrap_or_else(|| {
        let cols: Vec<&str> = columns.iter().map(|c| c.as_str()).collect();
        format!("index_{}_on_{}", table_name, cols.join("_and_"))
    });
    Ok(Index { name: Symbol::from(name), columns, unique, using, predicate })
}

/// Rails interpolates `using:` as an unquoted SQL identifier. Fold its
/// spelling as PostgreSQL does, and reject values that would not safely
/// round-trip through that schema DSL instead of silently using btree.
fn index_access_method(value: &Node<'_>, file: &str, table: &str) -> IngestResult<String> {
    let method = string_value(value).or_else(|| symbol_value(value)).ok_or_else(|| {
        IngestError::Unsupported {
            file: file.into(),
            message: format!("index access method for `{table}` must be a literal symbol or string"),
        }
    })?;
    let mut chars = method.chars();
    let valid_identifier = chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '$'));
    if !valid_identifier || method.len() > 63 {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "index access method for `{table}` must be an ASCII unquoted SQL identifier"
            ),
        });
    }
    Ok(method.to_ascii_lowercase())
}

/// `create_table NAME[, opts] do |t| … end` → (table key, Table).
/// Shared by the schema.rb walker and the migration fold.
fn table_from_create_table(
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    gaps: &mut Vec<IngestError>,
    dialect: GeneratedExpressionDialect,
) -> Option<(Symbol, Table)> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next();
    let table_name = first.as_ref().and_then(table_name_value)?;

    // Rails convention: every table has an implicit bigint primary-key `id`
    // unless `id: false` is passed to `create_table`. We honor that here by
    // synthesizing the column; the Ruby emitter's `primary_key` skip keeps
    // schema.rb round-trip-equal to the source.
    //
    // `id: :uuid` / `id: :string` names the key's TYPE, and
    // `primary_key: "identifier"` its NAME. Both used to be ignored, so
    // the table got an `id INTEGER PRIMARY KEY AUTOINCREMENT` the app
    // never declared and lost the column it did (#83).
    //
    // When the key has options beyond its type and default, Rails'
    // schema dumper writes them as a hash, `id: { type: :string, limit:
    // 32 }`, and leaves out `type:` for the default key. The hash used
    // to be read as no `id:` at all, so it always gave the default key.
    // `create_table`'s own `limit:` and `default:` are the key's too
    // (Rails merges the hash over them), so a hash's `limit:`, even
    // `nil`, wins over the outer one.
    let mut has_id = true;
    let mut id_type: Option<String> = None;
    let mut outer_limit: Option<u32> = None;
    let mut hash_limit: Option<Option<u32>> = None;
    let mut id_default = false;
    let mut id_name = "id".to_string();
    let limit_of = |node: &Node<'_>| integer_value(node).and_then(|n| u32::try_from(n).ok());
    for arg in args.arguments().iter().skip(1) {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            match key.as_str() {
                "id" => {
                    let value = assoc.value();
                    if let Some(false) = bool_value(&value) {
                        has_id = false;
                    } else if let Some(t) = symbol_value(&value) {
                        id_type = Some(t);
                    } else if let Some(hash) = value.as_hash_node() {
                        for el in hash.elements().iter() {
                            let Some(opt) = el.as_assoc_node() else { continue };
                            match symbol_value(&opt.key()).as_deref() {
                                Some("type") => id_type = name_value(&opt.value()),
                                Some("limit") => hash_limit = Some(limit_of(&opt.value())),
                                Some("default") => id_default = true,
                                _ => {}
                            }
                        }
                    }
                }
                "limit" => outer_limit = limit_of(&assoc.value()),
                "default" => id_default = true,
                "primary_key" => {
                    if let Some(n) = name_value(&assoc.value()) {
                        id_name = n;
                    }
                }
                _ => {}
            }
        }
    }

    let mut columns = Vec::new();
    let mut indexes: Vec<Index> = Vec::new();
    if has_id {
        let id_limit = hash_limit.unwrap_or(outer_limit);
        // `serial` and `bigserial` are the integer keys Postgres fills
        // from a sequence: Rails' PostgreSQL adapter makes `id: :integer`
        // a `serial`, and dumps it as `id: :serial`. An `integer` key
        // with no explicit `default:` has a sequence, and is a
        // `bigserial` when its `limit:` is 8 and a `serial` otherwise;
        // the sequence spends the `limit:`. Any other key is a column
        // of its type, and `column_with_type` reads its `limit:` as any
        // column's: an integer's 5 to 8 is a `bigint`, a string's is
        // its length.
        let (key_type, key_limit) = match id_type.as_deref() {
            Some("serial") => (Some("integer"), None),
            Some("bigserial") => (Some("bigint"), None),
            Some("integer") if !id_default && id_limit == Some(8) => (Some("bigint"), None),
            Some("integer") if !id_default => (Some("integer"), None),
            other => (other, id_limit),
        };
        let opts = ColumnOpts {
            nullable: Some(false),
            default: None,
            limit: key_limit,
            ..ColumnOpts::default()
        };
        let key = match key_type {
            None | Some("bigint") | Some("primary_key") => Ok(Column {
                name: Symbol::from(id_name.as_str()),
                col_type: ColumnType::BigInt,
                nullable: false,
                default: None,
                primary_key: true,
                generated: None,
                generated_text_compatible: None,
                generated_int4_compatible: None,
            }),
            Some(t) => column_with_type(t, id_name.clone(), &opts, &table_name, file),
        };
        match key {
            Ok(mut col) => {
                col.primary_key = true;
                // A non-integer key is a schema fact, not an ingest gap:
                // the DDL renders `TEXT PRIMARY KEY`, the analyzer types
                // `id`/`ids`/the finders from this column, and the
                // ruby-shape emit's insert writes and answers it. The
                // targets whose model layer still pins an integer id
                // say so at emit time, per target (`project::
                // target_files`), where "unsupported" can be true of one
                // lane and false of another (#90).
                columns.push(col);
            }
            Err(gap) => gaps.push(gap),
        }
    }
    if let Some(block_node) = call.block() {
        if let Some(block) = block_node.as_block_node() {
            if let Some(body) = block.body() {
                for stmt in flatten_statements(body) {
                    if let Some(call) = stmt.as_call_node() {
                        let call_name = constant_id_str(&call.name()).to_string();
                        if call_name == "index" {
                            match index_from_call(&call, &table_name, file) {
                                Ok(Some(idx)) => indexes.push(idx),
                                Ok(None) => {}
                                Err(gap) => gaps.push(gap),
                            }
                        } else if call_name == "timestamps" {
                            // Migration macro; schema.rb has these
                            // already materialized as two datetimes.
                            columns.extend(timestamp_columns());
                        } else {
                            match column_from_call(&call, &table_name, file) {
                                Ok(Some(col)) => columns.push(col),
                                Ok(None) => {}
                                Err(gap) => gaps.push(gap),
                            }
                        }
                    }
                }
            }
        }
    }

    // An index over a column the walk dropped (unsupported type,
    // ledgered above) cannot apply: sqlite refuses the seed at `no such
    // column`. The index goes with the column.
    indexes.retain(|idx| idx.columns.iter().all(|c| columns.iter().any(|col| col.name == *c)));
    let mut table = Table {
        name: Symbol::from(table_name.as_str()),
        columns,
        indexes,
        foreign_keys: vec![],
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
        table.indexes.retain(|idx| {
            idx.columns.iter().all(|name| table.columns.iter().any(|col| col.name == *name))
        });
    }
    Some((Symbol::from(table_name), table))
}

/// `create_virtual_table "name", "module", ["arg", …]` → (key, Table).
///
/// The arguments are the MODULE's own DSL, kept verbatim (see
/// [`crate::schema::VirtualModule`]). Those that name a column — the
/// ones with no `=` — are registered as Text columns so a query naming
/// them types, which is what fts5's `body` is. There is no `id`: a
/// virtual table's implicit key is `rowid`, and declaring one would put
/// a column in the DDL that fts5 rejects.
fn virtual_table_from_call(call: &ruby_prism::CallNode<'_>) -> Option<(Symbol, Table)> {
    let args = call.arguments()?;
    let mut it = args.arguments().iter();
    let name = name_value(&it.next()?)?;
    let module = name_value(&it.next()?)?;
    let mut module_args: Vec<String> = Vec::new();
    if let Some(list) = it.next().and_then(|a| a.as_array_node()) {
        for el in list.elements().iter() {
            if let Some(v) = string_value(&el).or_else(|| symbol_value(&el).map(|s| s.to_string()))
            {
                module_args.push(v);
            }
        }
    }
    let columns: Vec<Column> = module_args
        .iter()
        .filter(|a| !a.contains('='))
        .map(|a| Column {
            name: Symbol::from(a.as_str()),
            col_type: ColumnType::Text,
            nullable: true,
            default: None,
            primary_key: false,
            generated: None,
            generated_text_compatible: None,
            generated_int4_compatible: None,
        })
        .collect();
    Some((
        Symbol::from(name.clone()),
        Table {
            name: Symbol::from(name),
            columns,
            indexes: vec![],
            foreign_keys: vec![],
            virtual_module: Some(crate::schema::VirtualModule { module, args: module_args }),
        },
    ))
}

/// `create_view "name", sql_definition: "<SELECT …>"` → (view key,
/// Table). A SQL view's "columns" are the SELECT's output aliases, so
/// we register a Table whose columns are every `AS <alias>` in the
/// definition. The schema dumper writes each projected column with an
/// explicit `AS`, so the alias list is the column list.
fn view_from_create_view(
    call: &ruby_prism::CallNode<'_>,
    tables: &IndexMap<Symbol, Table>,
) -> Option<(Symbol, Table)> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next();
    let view_name = first.as_ref().and_then(name_value)?;

    let mut sql: Option<String> = None;
    for arg in args.arguments().iter().skip(1) {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            if key.as_str() == "sql_definition" {
                sql = string_value(&assoc.value());
            }
        }
    }
    let columns = view_columns_from_sql(&sql?, tables);
    if columns.is_empty() {
        return None;
    }

    Some((
        Symbol::from(view_name.clone()),
        Table {
            name: Symbol::from(view_name),
            columns,
            indexes: vec![],
            foreign_keys: vec![],
            virtual_module: None,
        },
    ))
}

/// Extract a view's columns from its SQL definition: each `<expr> AS
/// <alias>` in the SELECT list. When `<expr>` is a plain `table.column`
/// projection that is the whole select item, resolve its REAL type from
/// the already-parsed `tables` (a view just re-exposes table columns).
/// Only genuinely-computed items — comparisons (`a < b AS is_unread`)
/// and subqueries (`(select …) AS current_vote_vote`) — fall back to
/// `view_column_type`'s name heuristic, since they have no single source
/// column. Tokens keep their glued punctuation (a trailing `)` marks a
/// subquery, a preceding operator marks an expression) so the
/// direct-projection case is distinguishable without a full SQL parser.
fn view_columns_from_sql(sql: &str, tables: &IndexMap<Symbol, Table>) -> Vec<Column> {
    let mut columns = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let toks: Vec<&str> = sql.split_whitespace().collect();
    for i in 1..toks.len() {
        if !toks[i].eq_ignore_ascii_case("as") {
            continue;
        }
        let Some(alias_raw) = toks.get(i + 1) else { continue };
        let alias = alias_raw.trim_matches(|c| c == '`' || c == '"' || c == '\'' || c == ',');
        if alias.is_empty() || !alias.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        if !seen.insert(alias.to_string()) {
            continue;
        }
        // The source is a direct `table.column` projection only when the
        // token before `AS` is a clean column ref AND it's the whole
        // select item (preceded by `select` or a comma-terminated prior
        // alias) — otherwise it's the tail of an expression/subquery.
        let item_start = i < 2
            || toks[i - 2].eq_ignore_ascii_case("select")
            || toks[i - 2].ends_with(',');
        let resolved = if item_start {
            parse_col_ref(toks[i - 1]).and_then(|(t, c)| lookup_column_type(tables, &t, &c))
        } else {
            None
        };
        columns.push(Column {
            name: Symbol::from(alias),
            col_type: resolved.unwrap_or_else(|| view_column_type(alias)),
            nullable: true,
            default: None,
            primary_key: false,
            generated: None,
            generated_text_compatible: None,
            generated_int4_compatible: None,
        });
    }
    columns
}

/// Parse a clean `table.column` ref (optionally backtick/quote quoted)
/// into its parts. Returns None for anything with parens, operators, or
/// not exactly two dotted segments — i.e. anything that isn't a single
/// column projection.
fn parse_col_ref(tok: &str) -> Option<(String, String)> {
    if tok.contains('(') || tok.contains(')') {
        return None;
    }
    let cleaned: String = tok.chars().filter(|c| *c != '`' && *c != '"').collect();
    let mut parts = cleaned.split('.');
    let table = parts.next()?;
    let column = parts.next()?;
    if parts.next().is_some() || table.is_empty() || column.is_empty() {
        return None;
    }
    let ok = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok(table) || !ok(column) {
        return None;
    }
    Some((table.to_string(), column.to_string()))
}

/// Look up a column's declared type in the already-parsed schema.
fn lookup_column_type(
    tables: &IndexMap<Symbol, Table>,
    table: &str,
    column: &str,
) -> Option<ColumnType> {
    tables
        .get(&Symbol::from(table))?
        .columns
        .iter()
        .find(|c| c.name.as_str() == column)
        .map(|c| c.col_type.clone())
}

/// Fallback column type for a computed view column (no single source
/// column to consult), keyed on the alias name (AR conventions).
fn view_column_type(name: &str) -> ColumnType {
    if name == "id" || name.ends_with("_id") {
        ColumnType::Integer
    } else if name.ends_with("_at") {
        ColumnType::DateTime
    } else if name.starts_with("is_") || name.starts_with("has_") {
        ColumnType::Boolean
    } else {
        ColumnType::String { limit: None }
    }
}

/// The two columns `t.timestamps` expands to (Rails 5+ default:
/// `null: false`, no default value).
fn timestamp_columns() -> [Column; 2] {
    let col = |name: &str| Column {
        name: Symbol::from(name),
        col_type: ColumnType::DateTime,
        nullable: false,
        default: None,
        primary_key: false,
        generated: None,
        generated_text_compatible: None,
        generated_int4_compatible: None,
    };
    [col("created_at"), col("updated_at")]
}

/// A `references`/`belongs_to` column keeps the logical name (the
/// Reference type carries the target table) — same convention as the
/// schema.rb `t.references` path.
fn reference_column(name: &str) -> Column {
    Column {
        name: Symbol::from(name),
        col_type: ColumnType::Reference { table: TableRef(Symbol::from(name)) },
        nullable: true,
        default: None,
        primary_key: false,
        generated: None,
        generated_text_compatible: None,
        generated_int4_compatible: None,
    }
}

/// Kwarg options shared by `t.<type> NAME, opts` column calls and the
/// migration-verb forms (`add_column TABLE, NAME, TYPE, opts`).
#[derive(Default)]
struct ColumnOpts {
    nullable: Option<bool>,
    default: Option<String>,
    limit: Option<u32>,
    /// A present array option is non-scalar unless explicitly false or nil.
    array: bool,
}

// Not `string_value` alone: schema.rb dumps an integer, float or boolean default unquoted (`default: 0`, `default: true`).
fn default_value(node: &Node<'_>) -> Option<String> {
    if let Some(n) = integer_value(node) {
        return Some(n.to_string());
    }
    if let Some(f) = node.as_float_node() {
        return Some(f.value().to_string());
    }
    bool_value(node).map(|b| b.to_string()).or_else(|| string_value(node))
}

/// Collect column options while retaining array metadata omitted by the normalized column type.
fn parse_column_opts<'pr>(nodes: impl Iterator<Item = &'pr Node<'pr>>) -> ColumnOpts {
    let mut opts = ColumnOpts::default();
    for node in nodes {
        let Some(kh) = node.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = &assoc.value();
            match key.as_str() {
                "null" => opts.nullable = bool_value(value),
                "default" => opts.default = default_value(value),
                "limit" => {
                    if let Some(n) = integer_value(value) {
                        if n >= 0 {
                            opts.limit = Some(n as u32);
                        }
                    }
                }
                "array" => {
                    opts.array = match bool_value(value) {
                        Some(enabled) => enabled,
                        None => value.as_nil_node().is_none(),
                    };
                }
                _ => {}
            }
        }
    }
    opts
}

/// Whether the option list names any Rails-generated-column field.
/// `as:` and `stored:` must never be ignored and turn a computed column
/// into an ordinary writable column. `type:` alone is not a generated
/// column marker: ordinary references/belongs_to declarations may carry
/// it as an option.
fn has_generated_column_options(nodes: &[Node<'_>]) -> bool {
    nodes.iter().any(|node| {
        node.as_keyword_hash_node().is_some_and(|hash| {
            hash.elements().iter().any(|element| {
                element.as_assoc_node().is_some_and(|assoc| {
                    matches!(symbol_value(&assoc.key()).as_deref(), Some("as" | "stored"))
                })
            })
        }) || node.as_hash_node().is_some_and(|hash| {
            hash.elements().iter().any(|element| {
                element.as_assoc_node().is_some_and(|assoc| {
                    matches!(symbol_value(&assoc.key()).as_deref(), Some("as" | "stored"))
                })
            })
        })
    })
}

/// An option splat can hide `as:`, `stored:`, or `type:` from this schema
/// fold. Do not let the ordinary-column path silently discard that metadata.
fn has_column_option_splats(nodes: &[Node<'_>]) -> bool {
    nodes.iter().any(|node| {
        node.as_assoc_splat_node().is_some()
            || node.as_keyword_hash_node().is_some_and(|hash| {
                hash.elements()
                    .iter()
                    .any(|element| element.as_assoc_splat_node().is_some())
            })
            || node.as_hash_node().is_some_and(|hash| {
                hash.elements()
                    .iter()
                    .any(|element| element.as_assoc_splat_node().is_some())
            })
    })
}

/// Parse the strict option set for a generated schema/migration column.
/// For `t.virtual`, `fallback_type` is absent and `type:` is required.
/// For `add_column` with an ordinary type and `as:`, the positional type
/// is retained and a conflicting `type:` is rejected.
fn generated_column_from_options(
    col_name: String,
    table: &str,
    file: &str,
    fallback_type: Option<&str>,
    nodes: &[Node<'_>],
) -> Result<Column, IngestError> {
    let unsupported = |message: String| IngestError::Unsupported { file: file.into(), message };
    let mut type_name = fallback_type.map(str::to_string);
    let mut expression: Option<String> = None;
    let mut stored: Option<bool> = None;
    let mut opts = ColumnOpts::default();
    let mut seen: Vec<String> = Vec::new();

    for node in nodes {
        let Some(hash) = node.as_keyword_hash_node() else {
            return Err(unsupported(format!(
                "generated column options for {table}.{col_name} must be keyword arguments"
            )));
        };
        for element in hash.elements().iter() {
            let Some(assoc) = element.as_assoc_node() else {
                return Err(unsupported(format!(
                    "generated column options for {table}.{col_name} are not supported"
                )));
            };
            let Some(key) = symbol_value(&assoc.key()) else {
                return Err(unsupported(format!(
                    "generated column option key for {table}.{col_name} must be a symbol"
                )));
            };
            if seen.iter().any(|existing| existing == &key) {
                return Err(unsupported(format!(
                    "generated column option `{key}` is repeated for {table}.{col_name}"
                )));
            }
            seen.push(key.clone());

            let value = assoc.value();
            match key.as_str() {
                "type" => {
                    if type_name.is_some() {
                        return Err(unsupported(format!(
                            "generated column {table}.{col_name} has both a positional type and `type:`"
                        )));
                    }
                    type_name = name_value(&value);
                    if type_name.is_none() {
                        return Err(unsupported(format!(
                            "generated column type for {table}.{col_name} must be a string or symbol"
                        )));
                    }
                }
                "as" => {
                    expression = string_value(&value);
                    if expression.is_none() {
                        return Err(unsupported(format!(
                            "generated expression for {table}.{col_name} must be a string literal"
                        )));
                    }
                }
                "stored" => {
                    stored = bool_value(&value);
                    if stored.is_none() {
                        return Err(unsupported(format!(
                            "generated storage mode for {table}.{col_name} must be a boolean"
                        )));
                    }
                }
                "null" => {
                    opts.nullable = bool_value(&value);
                    if opts.nullable.is_none() {
                        return Err(unsupported(format!(
                            "generated column nullability for {table}.{col_name} must be a boolean"
                        )));
                    }
                }
                "limit" => {
                    let Some(limit) = integer_value(&value).and_then(|n| u32::try_from(n).ok()) else {
                        return Err(unsupported(format!(
                            "generated column limit for {table}.{col_name} must be a non-negative integer"
                        )));
                    };
                    opts.limit = Some(limit);
                }
                "default" => {
                    return Err(unsupported(format!(
                        "generated columns cannot also have a default: {table}.{col_name}"
                    )));
                }
                "primary_key" => {
                    if bool_value(&value).unwrap_or(true) {
                        return Err(unsupported(format!(
                            "generated columns cannot be primary keys: {table}.{col_name}"
                        )));
                    }
                }
                _ => {
                    return Err(unsupported(format!(
                        "generated column option `{key}` is unsupported for {table}.{col_name}"
                    )));
                }
            }
        }
    }

    let type_name = type_name.ok_or_else(|| {
        unsupported(format!("generated column type is missing for {table}.{col_name}"))
    })?;
    let expression = expression.ok_or_else(|| {
        unsupported(format!("generated expression is missing for {table}.{col_name}"))
    })?;
    let mut column = column_with_type(&type_name, col_name.clone(), &opts, table, file)?;
    column.generated = Some(GeneratedColumn {
        expression,
        storage: if stored.unwrap_or(false) {
            GeneratedColumnStorage::Stored
        } else {
            GeneratedColumnStorage::Virtual
        },
    });
    Ok(column)
}

/// A column-type name (`t.<type>` / `add_column …, :<type>`) to its
/// `ColumnType`. The Postgres-only types map to their SQLite storage:
/// `uuid`, `json`, and `jsonb` retain distinct schema variants even when
/// their shared SQLite/runtime representation uses text;
/// `citext` is text; `timestamptz` is a datetime; the network types
/// and a PG `enum` are strings. `timestamp` is Rails' own alias for
/// `datetime` (`TableDefinition#timestamp`), which a MySQL-backed
/// app's `schema.rb` dumps for a `TIMESTAMP` column — lobsters'
/// `story_texts.created_at`. A type not listed is an error, not a
/// silent drop — its index would still be emitted and the DDL would
/// not apply (#83).
fn column_with_type(
    type_name: &str,
    col_name: String,
    opts: &ColumnOpts,
    table: &str,
    file: &str,
) -> Result<Column, IngestError> {
    let col_type = match type_name {
        // An integer's `limit:` is its size in bytes, and Rails' PostgreSQL
        // and MySQL adapters make 5 to 8 a `bigint`. Rails 4.2 and earlier
        // dumped a bigint column as `t.integer …, limit: 8`, and
        // solid_cache's and solid_cable's schemas still write it for their
        // hash columns.
        "integer" if matches!(opts.limit, Some(5..=8)) => ColumnType::BigInt,
        // `smallint` is the PostgreSQL adapter's native type name, which
        // `t.column :island, :smallint` passes through; structure.sql's
        // `smallint` reads alike.
        "integer" | "smallint" => ColumnType::Integer,
        "bigint" => ColumnType::BigInt,
        "float" => ColumnType::Float,
        "decimal" | "numeric" => ColumnType::Decimal { precision: None, scale: None },
        "string" | "inet" | "cidr" | "macaddr" | "enum" => ColumnType::String { limit: opts.limit },
        "text" | "citext" => ColumnType::Text,
        "boolean" => ColumnType::Boolean,
        "date" => ColumnType::Date,
        "datetime" | "timestamp" | "timestamptz" => ColumnType::DateTime,
        "time" => ColumnType::Time,
        "binary" => ColumnType::Binary,
        "json" => ColumnType::Json,
        "jsonb" => ColumnType::Jsonb,
        "uuid" => ColumnType::Uuid,
        "references" | "belongs_to" => {
            ColumnType::Reference { table: TableRef(Symbol::from(col_name.as_str())) }
        }
        _ => {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!(
                    "column dropped: {table}.{col_name} has unsupported type `{type_name}`"
                ),
            })
        }
    };

    // Several PostgreSQL-only aliases are represented as ordinary
    // strings/text for existing application typing. Preserve just the
    // negative provenance needed by the generated-expression validator:
    // these source types do not have the portable text semantics of
    // Rails `string`/`text` columns.
    let generated_text_compatible = (opts.array || matches!(
        type_name,
        "inet" | "cidr" | "macaddr" | "enum" | "citext"
    ) || (type_name == "text" && opts.limit.is_some()))
    .then_some(false);
    // Rails' PostgreSQL adapter maps integer limits 1 and 2 to smallint,
    // 3 and 4 to integer, and 5 through 8 to bigint. The shared ordinary
    // IR keeps limits 1/2 as Integer, so preserve only the negative fact
    // needed to reject them as generated int4 results. Invalid sizes also
    // cannot be treated as exact int4; 5..=8 are already BigInt above.
    let generated_int4_compatible = if type_name == "integer" {
        match opts.limit {
            Some(0 | 1 | 2) => Some(false),
            Some(3 | 4) | None | Some(5..=8) => None,
            Some(_) => Some(false),
        }
    } else {
        None
    };

    Ok(Column {
        name: Symbol::from(col_name),
        col_type,
        nullable: opts.nullable.unwrap_or(true),
        default: opts.default.clone(),
        primary_key: false,
        generated: None,
        generated_text_compatible,
        generated_int4_compatible,
    })
}

/// `Ok(None)` when the call is not a `t.<type> name` column line at
/// all; `Err` when it is one whose type has no mapping.
fn column_from_call(
    call: &ruby_prism::CallNode<'_>,
    table: &str,
    file: &str,
) -> Result<Option<Column>, IngestError> {
    // Expected: t.string "title", null: false  (schema.rb)
    //       or: t.string :title               (migration)
    // Receiver is a LocalVariableReadNode named "t".
    let Some(recv) = call.receiver() else { return Ok(None) };
    if recv.as_local_variable_read_node().is_none() {
        return Ok(None);
    }

    let col_type_name = constant_id_str(&call.name()).to_string();
    if col_type_name == "virtual" {
        let Some(args_node) = call.arguments() else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("generated column declaration has no arguments in {table}"),
            });
        };
        let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
        let Some(col_name) = args.first().and_then(name_value) else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("generated column declaration has no name in {table}"),
            });
        };
        if has_column_option_splats(&args[1..]) {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!(
                    "column options for {table}.{col_name} cannot use keyword splats because generated-column metadata may be hidden"
                ),
            });
        }
        return generated_column_from_options(col_name, table, file, None, &args[1..]).map(Some);
    }
    // `t.<constraint>` lines are table-level declarations, not columns;
    // the fold does not model them and they cost the DDL nothing.
    if matches!(
        col_type_name.as_str(),
        "check_constraint" | "foreign_key" | "exclusion_constraint" | "unique_constraint"
    ) {
        return Ok(None);
    }
    let Some(args_node) = call.arguments() else { return Ok(None) };
    let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
    let Some(col_name) = args.first().and_then(name_value) else { return Ok(None) };
    if has_column_option_splats(&args[1..]) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "column options for {table}.{col_name} cannot use keyword splats because generated-column metadata may be hidden"
            ),
        });
    }
    if has_generated_column_options(&args[1..]) {
        return Err(IngestError::Unsupported {
            file: file.into(),
            message: format!(
                "generated column options on `t.{col_type_name}` are unsupported in {table}; use `t.virtual`"
            ),
        });
    }
    // `t.column :name, :type, …` is the generic form of `t.<type> :name, …`
    // (`TableDefinition#column`).
    if col_type_name == "column" {
        let Some(type_name) = args.get(1).and_then(name_value) else {
            return Err(IngestError::Unsupported {
                file: file.into(),
                message: format!("column dropped: {table}.{col_name} has no literal type"),
            });
        };
        let opts = parse_column_opts(args.iter().skip(2));
        return column_with_type(&type_name, col_name, &opts, table, file).map(Some);
    }
    let opts = parse_column_opts(args.iter().skip(1));
    column_with_type(&col_type_name, col_name, &opts, table, file).map(Some)
}

fn index_from_call(
    call: &ruby_prism::CallNode<'_>,
    table_name: &str,
    file: &str,
) -> IngestResult<Option<Index>> {
    // Expected: t.index ["article_id"], name: "...", unique: true
    //       or: t.index :article_id  (migration single-column form)
    let Some(recv) = call.receiver() else { return Ok(None) };
    if recv.as_local_variable_read_node().is_none() {
        return Ok(None);
    }

    let Some(args_node) = call.arguments() else { return Ok(None) };
    let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
    let Some(columns) = args.first().map(column_name_list) else { return Ok(None) };
    if columns.is_empty() {
        return Ok(None);
    }
    build_index(table_name, columns, args.iter().skip(1), file).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(files: &[&str]) -> Schema {
        let mut schema = Schema::default();
        for (i, src) in files.iter().enumerate() {
            ingest_migration(src.as_bytes(), &format!("{i}_migration.rb"), &mut schema)
                .expect("fold");
        }
        schema
    }

    fn col_names(schema: &Schema, table: &str) -> Vec<String> {
        schema.tables[&Symbol::from(table)]
            .columns
            .iter()
            .map(|c| c.name.as_str().to_string())
            .collect()
    }

    #[test]
    fn create_table_with_symbols_and_timestamps() {
        let schema = fold(&[r#"
            class CreateClips < ActiveRecord::Migration[8.1]
              def change
                create_table :clips do |t|
                  t.string :name
                  t.text :transcript
                  t.float :duration

                  t.timestamps
                end
              end
            end
        "#]);
        assert_eq!(
            col_names(&schema, "clips"),
            ["id", "name", "transcript", "duration", "created_at", "updated_at"]
        );
        let clips = &schema.tables[&Symbol::from("clips")];
        assert!(matches!(clips.columns[3].col_type, ColumnType::Float));
        assert!(!clips.columns[4].nullable, "timestamps are null: false");
    }

    #[test]
    fn a_non_literal_column_name_is_a_gap_not_a_skip() {
        let mut schema = fold(&[r#"
            class CreateIdx < ActiveRecord::Migration[8.1]
              def change
                create_table :idx do |t|
                  t.string :col_1
                end
              end
            end
        "#]);
        let err = ingest_migration(
            br#"
            class DropSlots < ActiveRecord::Migration[8.1]
              def up
                (1..1).each { |n| remove_column(:idx, :"col_#{n}") }
              end
            end
        "#,
            "drop.rb",
            &mut schema,
        )
        .expect_err("a non-literal name cannot apply");
        assert!(err.to_string().contains("`remove_column` argument 2 is not a literal"), "{err}");
    }

    #[test]
    fn generic_column_with_a_native_type() {
        // `t.column(:island, :smallint, …)`.
        let schema = fold(&[r#"
            class CreateCounts < ActiveRecord::Migration[8.1]
              def change
                create_table :counts do |t|
                  t.column(:island, :smallint, null: false)
                  t.column "label", "string"
                end
              end
            end
        "#]);
        assert_eq!(col_names(&schema, "counts"), ["id", "island", "label"]);
        let counts = &schema.tables[&Symbol::from("counts")];
        assert!(matches!(counts.columns[1].col_type, ColumnType::Integer));
        assert!(!counts.columns[1].nullable);
        assert!(matches!(counts.columns[2].col_type, ColumnType::String { .. }));
    }

    #[test]
    fn incremental_verbs_fold_in_order() {
        let schema = fold(&[
            r#"
            class CreatePosts < ActiveRecord::Migration[8.0]
              def change
                create_table :posts do |t|
                  t.string :titel
                end
              end
            end
            "#,
            r#"
            class FixPosts < ActiveRecord::Migration[8.0]
              def change
                rename_column :posts, :titel, :title
                add_column :posts, :body, :text
                add_column :posts, :draft, :boolean, default: "true", null: false
                add_reference :posts, :author
              end
            end
            "#,
            r#"
            class TrimPosts < ActiveRecord::Migration[8.0]
              def change
                remove_column :posts, :draft
              end
            end
            "#,
        ]);
        assert_eq!(col_names(&schema, "posts"), ["id", "title", "body", "author"]);
        let posts = &schema.tables[&Symbol::from("posts")];
        assert!(matches!(posts.columns[3].col_type, ColumnType::Reference { .. }));
    }

    #[test]
    fn drop_and_rename_table() {
        let schema = fold(&[
            r#"
            class A < ActiveRecord::Migration[8.0]
              def change
                create_table :tmp do |t|
                  t.string :x
                end
                create_table :olds do |t|
                  t.string :y
                end
              end
            end
            "#,
            r#"
            class B < ActiveRecord::Migration[8.0]
              def change
                drop_table :tmp
                rename_table :olds, :news
              end
            end
            "#,
        ]);
        assert!(!schema.tables.contains_key(&Symbol::from("tmp")));
        assert!(!schema.tables.contains_key(&Symbol::from("olds")));
        assert_eq!(col_names(&schema, "news"), ["id", "y"]);
    }

    #[test]
    fn up_method_used_when_no_change_down_ignored() {
        let schema = fold(&[r#"
            class Legacy < ActiveRecord::Migration[6.0]
              def up
                create_table :things do |t|
                  t.string :name
                end
              end

              def down
                drop_table :things
              end
            end
        "#]);
        assert!(schema.tables.contains_key(&Symbol::from("things")));
    }

    #[test]
    fn unsupported_verb_errors_with_guidance() {
        let mut schema = Schema::default();
        let err = ingest_migration(
            b"class X < ActiveRecord::Migration[8.0]\n  def change\n    execute \"DROP TABLE foo\"\n  end\nend",
            "1_x.rb",
            &mut schema,
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("execute"), "names the verb: {msg}");
        assert!(msg.contains("rails db:migrate"), "points at the fix: {msg}");
    }

    #[test]
    fn backfill_ruby_is_ignored() {
        // Receiver-bearing calls and unknown receiver-less helpers are
        // not schema-affecting; the fold must not trip on them.
        let schema = fold(&[r#"
            class Backfill < ActiveRecord::Migration[8.0]
              def change
                create_table :users do |t|
                  t.string :email
                end
                say "backfilling"
              end
            end
        "#]);
        assert_eq!(col_names(&schema, "users"), ["id", "email"]);
    }

    /// Index metadata from both table-block `t.index` and migration
    /// `add_index` calls survives the same schema fold.
    #[test]
    fn migration_index_options_keep_predicate_and_access_method() {
        let schema = fold(&[r#"
            class CreateTokens < ActiveRecord::Migration[8.1]
              def change
                create_table :tokens do |t|
                  t.bigint :user_id, null: false
                  t.datetime :revoked_at
                  t.jsonb :payload
                  t.index :user_id, unique: true, where: "revoked_at IS NULL", name: "live"
                end
                add_index :tokens, :revoked_at, where: "revoked_at IS NOT NULL"
                add_index :tokens, [:user_id, :revoked_at]
                add_index :tokens, :payload, using: :gin, name: "payload_gin"
              end
            end
        "#]);
        let indexes: Vec<(&str, bool, Option<&str>, Option<&str>)> = schema.tables[&Symbol::from("tokens")]
            .indexes
            .iter()
            .map(|i| (i.name.as_str(), i.unique, i.predicate.as_deref(), i.using.as_deref()))
            .collect();
        assert_eq!(
            indexes,
            [
                ("live", true, Some("revoked_at IS NULL"), None),
                ("index_tokens_on_revoked_at", false, Some("revoked_at IS NOT NULL"), None),
                ("index_tokens_on_user_id_and_revoked_at", false, None, None),
                ("payload_gin", false, None, Some("gin")),
            ]
        );
    }

    /// The fold has only a predicate's text, so it refuses to rename or
    /// remove a column the predicate names, rather than render a `WHERE`
    /// naming a column that is gone. A column it doesn't name is fine.
    #[test]
    fn a_column_a_predicate_names_is_not_renamed_or_removed() {
        let create = r#"
            class CreateTokens < ActiveRecord::Migration[8.1]
              def change
                create_table :tokens do |t|
                  t.bigint :user_id, null: false
                  t.datetime :revoked_at
                  t.string :label
                  t.index :user_id, unique: true, where: "\"revoked_at\" IS NULL", name: "live"
                end
              end
            end
        "#;
        for verb in ["rename_column :tokens, :revoked_at, :archived_at", "remove_column :tokens, :revoked_at"] {
            let mut schema = Schema::default();
            ingest_migration(create.as_bytes(), "1_create.rb", &mut schema).expect("create");
            let change = format!(
                "class Change < ActiveRecord::Migration[8.1]\n  def change\n    {verb}\n  end\nend\n"
            );
            let err = ingest_migration(change.as_bytes(), "2_change.rb", &mut schema).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("tokens.revoked_at") && msg.contains("index `live`"), "{msg}");
            assert!(msg.contains("rails db:migrate"), "{msg}");
        }
        let schema = fold(&[
            create,
            "class Change < ActiveRecord::Migration[8.1]\n  def change\n    rename_column :tokens, :label, :title\n  end\nend\n",
        ]);
        assert_eq!(col_names(&schema, "tokens"), ["id", "user_id", "revoked_at", "title"]);

        // A quoted name is one identifier, spaces and `""` included; a
        // string literal's words are not names.
        assert!(predicate_names("(\"revoked at\" IS NULL)", "revoked at"));
        assert!(predicate_names("(\"say \"\"hi\"\"\" <> '')", "say \"hi\""));
        assert!(!predicate_names("(\"revoked at\" IS NULL)", "revoked"));
        assert!(!predicate_names("(state = 'revoked_at')", "revoked_at"));
        assert!(predicate_names("(REVOKED_AT IS NULL)", "revoked_at"));
    }

    /// An integer's `limit:` is its size in bytes: 5 to 8 is a `bigint`,
    /// in `t.integer`, `add_column` and `change_column` alike, and 4 or
    /// no limit stays `integer`.
    #[test]
    fn an_eight_byte_integer_is_a_bigint() {
        let schema = fold(&[
            r#"
            class CreateEntries < ActiveRecord::Migration[8.1]
              def change
                create_table :entries do |t|
                  t.integer :key_hash, limit: 8, null: false
                  t.integer :five, limit: 5
                  t.integer :seven, limit: 7
                  t.integer :byte_size, limit: 4, null: false
                  t.integer :position
                end
                add_column :entries, :channel_hash, :integer, limit: 8
                add_column :entries, :widened, :integer
                add_column :entries, :narrowed, :integer, limit: 8
              end
            end
            "#,
            r#"
            class ResizeEntries < ActiveRecord::Migration[8.1]
              def change
                change_column :entries, :widened, :integer, limit: 8
                change_column :entries, :narrowed, :integer, limit: 4
              end
            end
            "#,
        ]);
        let mut types: Vec<(&str, &ColumnType)> = schema.tables[&Symbol::from("entries")]
            .columns
            .iter()
            .map(|c| (c.name.as_str(), &c.col_type))
            .collect();
        types.sort_by_key(|(name, _)| *name);
        assert_eq!(
            types,
            [
                ("byte_size", &ColumnType::Integer),
                ("channel_hash", &ColumnType::BigInt),
                ("five", &ColumnType::BigInt),
                ("id", &ColumnType::BigInt),
                ("key_hash", &ColumnType::BigInt),
                ("narrowed", &ColumnType::Integer),
                ("position", &ColumnType::Integer),
                ("seven", &ColumnType::BigInt),
                ("widened", &ColumnType::BigInt),
            ]
        );
    }

    #[test]
    fn schema_rb_string_form_still_parses() {
        let schema = ingest_schema(
            br#"
            ActiveRecord::Schema[8.0].define(version: 1) do
              create_table "articles", force: :cascade do |t|
                t.string "title"
                t.datetime "created_at", null: false
              end
            end
            "#,
            "db/schema.rb",
        )
        .unwrap();
        assert_eq!(col_names(&schema, "articles"), ["id", "title", "created_at"]);
    }
}
