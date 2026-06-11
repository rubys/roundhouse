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

use ruby_prism::{Node, parse};

use crate::schema::{Column, ColumnType, Index, Schema, Table};
use crate::{Symbol, TableRef};

use super::util::{
    bool_value, constant_id_str, find_first_class, flatten_statements, integer_value,
    string_value, symbol_value, walk_calls,
};
use super::{IngestError, IngestResult};

pub fn ingest_schema(source: &[u8], _file: &str) -> IngestResult<Schema> {
    let result = parse(source);
    let root = result.node();

    let mut schema = Schema::default();
    walk_calls(&root, &mut |call| {
        if constant_id_str(&call.name()) != "create_table" {
            return;
        }
        if let Some((name, table)) = table_from_create_table(call) {
            schema.tables.insert(name, table);
        }
    });

    Ok(schema)
}

/// Fold one `db/migrate/*.rb` file into `schema` — the fallback schema
/// source when `db/schema.rb` is absent. The caller iterates files in
/// filename order (timestamp prefixes sort chronologically).
///
/// Only the migration's `change` method is replayed (`up` when no
/// `change` exists; `down` is never touched). Schema-mutating verbs we
/// can't fold deterministically (`change_table`, `execute`, raw-SQL
/// shapes — see `UNSUPPORTED_VERBS`) error with a pointer to
/// `rails db:migrate`, which materializes the schema.rb this fallback
/// substitutes for. Receiver-less calls that aren't recognized verbs
/// are ignored: migrations legitimately contain arbitrary Ruby (data
/// backfills, `say`, …) that doesn't affect the schema.
pub fn ingest_migration(source: &[u8], file: &str, schema: &mut Schema) -> IngestResult<()> {
    let result = parse(source);
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
        if let Err(e) = apply_migration_verb(&verb, call, file, schema) {
            err = Some(e);
        }
    });
    match err {
        Some(e) => Err(e),
        None => Ok(()),
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

fn apply_migration_verb(
    verb: &str,
    call: &ruby_prism::CallNode<'_>,
    file: &str,
    schema: &mut Schema,
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
    let arg_name = |i: usize| args.get(i).and_then(name_value);

    match verb {
        "create_table" => {
            if let Some((name, table)) = table_from_create_table(call) {
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
            if let (Some(t), Some(c), Some(ty)) = (arg_name(0), arg_name(1), arg_name(2)) {
                let opts = parse_column_opts(args.iter().skip(3));
                if let Some(col) = column_with_type(&ty, c, &opts) {
                    if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                        // change_column replaces; add_column after a
                        // replace-shaped history stays idempotent.
                        table.columns.retain(|x| x.name != col.name);
                        table.columns.push(col);
                    }
                }
            }
        }
        "remove_column" => {
            if let (Some(t), Some(c)) = (arg_name(0), arg_name(1)) {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    table.columns.retain(|x| x.name.as_str() != c);
                }
            }
        }
        "rename_column" => {
            if let (Some(t), Some(old), Some(new)) = (arg_name(0), arg_name(1), arg_name(2)) {
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
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
            // Positional literal or `from:`/`to:` kwargs; only string
            // literals are retained (parity with the schema.rb parser).
            if let (Some(t), Some(c)) = (arg_name(0), arg_name(1)) {
                let positional = args.get(2).and_then(string_value);
                let to_kwarg = kwarg_value(args.iter().skip(2), "to").and_then(|v| string_value(&v));
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    for col in &mut table.columns {
                        if col.name.as_str() == c {
                            col.default = to_kwarg.clone().or_else(|| positional.clone());
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
                if let Some(table) = schema.tables.get_mut(&Symbol::from(t)) {
                    table.columns.retain(|x| x.name.as_str() != name);
                }
            }
        }
        "add_index" => {
            if let (Some(t), Some(columns)) = (arg_name(0), args.get(1).map(column_name_list)) {
                if !columns.is_empty() {
                    if let Some(table) = schema.tables.get_mut(&Symbol::from(t.clone())) {
                        table.indexes.push(build_index(&t, columns, args.iter().skip(2)));
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
        // Foreign keys and extensions don't affect column typing; the
        // schema.rb parser skips them too.
        "add_foreign_key" | "remove_foreign_key" | "enable_extension" | "disable_extension" => {}
        // Anything else receiver-less is arbitrary migration Ruby
        // (`say`, backfill helpers) — not schema-affecting.
        _ => {}
    }
    Ok(())
}

/// schema.rb writes string literals (`create_table "clips"`); hand-
/// written migrations write symbols (`create_table :clips`). Accept
/// both anywhere a table/column name is read.
fn name_value(node: &Node<'_>) -> Option<String> {
    string_value(node).or_else(|| symbol_value(node))
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
) -> Index {
    let mut explicit_name: Option<String> = None;
    let mut unique = false;
    for node in kwarg_nodes {
        let Some(kh) = node.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            let value = &assoc.value();
            match key.as_str() {
                "name" => explicit_name = string_value(value),
                "unique" => unique = bool_value(value).unwrap_or(false),
                _ => {}
            }
        }
    }
    let name = explicit_name.unwrap_or_else(|| {
        let cols: Vec<&str> = columns.iter().map(|c| c.as_str()).collect();
        format!("index_{}_on_{}", table_name, cols.join("_and_"))
    });
    Index { name: Symbol::from(name), columns, unique }
}

/// `create_table NAME[, opts] do |t| … end` → (table key, Table).
/// Shared by the schema.rb walker and the migration fold.
fn table_from_create_table(call: &ruby_prism::CallNode<'_>) -> Option<(Symbol, Table)> {
    let args = call.arguments()?;
    let first = args.arguments().iter().next();
    let table_name = first.as_ref().and_then(name_value)?;

    // Rails convention: every table has an implicit bigint primary-key `id`
    // unless `id: false` is passed to `create_table`. We honor that here by
    // synthesizing the column; the Ruby emitter's `primary_key` skip keeps
    // schema.rb round-trip-equal to the source.
    let mut has_id = true;
    for arg in args.arguments().iter().skip(1) {
        let Some(kh) = arg.as_keyword_hash_node() else { continue };
        for el in kh.elements().iter() {
            let Some(assoc) = el.as_assoc_node() else { continue };
            let Some(key) = symbol_value(&assoc.key()) else { continue };
            if key.as_str() == "id" {
                if let Some(false) = bool_value(&assoc.value()) {
                    has_id = false;
                }
            }
        }
    }

    let mut columns = Vec::new();
    let mut indexes: Vec<Index> = Vec::new();
    if has_id {
        columns.push(Column {
            name: Symbol::from("id"),
            col_type: ColumnType::BigInt,
            nullable: false,
            default: None,
            primary_key: true,
        });
    }
    if let Some(block_node) = call.block() {
        if let Some(block) = block_node.as_block_node() {
            if let Some(body) = block.body() {
                for stmt in flatten_statements(body) {
                    if let Some(call) = stmt.as_call_node() {
                        let call_name = constant_id_str(&call.name()).to_string();
                        if call_name == "index" {
                            if let Some(idx) = index_from_call(&call, &table_name) {
                                indexes.push(idx);
                            }
                        } else if call_name == "timestamps" {
                            // Migration macro; schema.rb has these
                            // already materialized as two datetimes.
                            columns.extend(timestamp_columns());
                        } else if let Some(col) = column_from_call(&call) {
                            columns.push(col);
                        }
                    }
                }
            }
        }
    }

    Some((
        Symbol::from(table_name.clone()),
        Table {
            name: Symbol::from(table_name),
            columns,
            indexes,
            foreign_keys: vec![],
        },
    ))
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
    }
}

/// Kwarg options shared by `t.<type> NAME, opts` column calls and the
/// migration-verb forms (`add_column TABLE, NAME, TYPE, opts`).
#[derive(Default)]
struct ColumnOpts {
    nullable: Option<bool>,
    default: Option<String>,
    limit: Option<u32>,
}

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
                "default" => opts.default = string_value(value),
                "limit" => {
                    if let Some(n) = integer_value(value) {
                        if n >= 0 {
                            opts.limit = Some(n as u32);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    opts
}

fn column_with_type(type_name: &str, col_name: String, opts: &ColumnOpts) -> Option<Column> {
    let col_type = match type_name {
        "integer" => ColumnType::Integer,
        "bigint" => ColumnType::BigInt,
        "float" => ColumnType::Float,
        "decimal" => ColumnType::Decimal { precision: None, scale: None },
        "string" => ColumnType::String { limit: opts.limit },
        "text" => ColumnType::Text,
        "boolean" => ColumnType::Boolean,
        "date" => ColumnType::Date,
        "datetime" => ColumnType::DateTime,
        "time" => ColumnType::Time,
        "binary" => ColumnType::Binary,
        "json" => ColumnType::Json,
        "references" | "belongs_to" => {
            ColumnType::Reference { table: TableRef(Symbol::from(col_name.as_str())) }
        }
        _ => return None,
    };

    Some(Column {
        name: Symbol::from(col_name),
        col_type,
        nullable: opts.nullable.unwrap_or(true),
        default: opts.default.clone(),
        primary_key: false,
    })
}

fn column_from_call(call: &ruby_prism::CallNode<'_>) -> Option<Column> {
    // Expected: t.string "title", null: false  (schema.rb)
    //       or: t.string :title               (migration)
    // Receiver is a LocalVariableReadNode named "t".
    let recv = call.receiver()?;
    recv.as_local_variable_read_node()?;

    let col_type_name = constant_id_str(&call.name()).to_string();
    let args_node = call.arguments()?;
    let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
    let col_name = args.first().and_then(name_value)?;
    let opts = parse_column_opts(args.iter().skip(1));
    column_with_type(&col_type_name, col_name, &opts)
}

fn index_from_call(call: &ruby_prism::CallNode<'_>, table_name: &str) -> Option<Index> {
    // Expected: t.index ["article_id"], name: "...", unique: true
    //       or: t.index :article_id  (migration single-column form)
    let recv = call.receiver()?;
    recv.as_local_variable_read_node()?;

    let args_node = call.arguments()?;
    let args: Vec<Node<'_>> = args_node.arguments().iter().collect();
    let columns = args.first().map(column_name_list)?;
    if columns.is_empty() {
        return None;
    }
    Some(build_index(table_name, columns, args.iter().skip(1)))
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
