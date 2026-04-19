//! Target-neutral persistence lowering.
//!
//! Captures everything an ActiveRecord `save` / `destroy` / `find` /
//! `count` needs at the SQL layer: the INSERT / UPDATE / DELETE /
//! SELECT strings, the column projection for rehydrating a row, the
//! implicit `belongs_to` existence checks, and the
//! `dependent: :destroy` cascade targets. Per-target emitters wrap
//! the SQL strings in whatever DB-binding API the target uses
//! (rusqlite, better-sqlite3, Python's sqlite3 module, etc.).
//!
//! SQLite-specific today: placeholder syntax (`?1`, `?2`, …), the
//! assumption that the primary key is named `id` and autoincrements
//! via `INTEGER PRIMARY KEY AUTOINCREMENT`, and the absence of schema
//! FK constraints. A later `Dialect` enum can render per-engine
//! variants without changing the consumer shape.

use std::fmt::Write;

use crate::dialect::{Association, Dependent, Model};
use crate::ident::{ClassId, Symbol};
use crate::App;

/// SQL-level view of a model's persistence surface. Consumed by every
/// target emitter's `save` / `destroy` / `find` / `count` rendering.
#[derive(Clone, Debug, PartialEq)]
pub struct LoweredPersistence {
    pub class: ClassId,
    pub table: Symbol,
    /// Every column in declaration order (IndexMap iteration). `id`
    /// is included; emitters use this list when building the SELECT
    /// projection so the struct hydrates in field order.
    pub columns: Vec<Symbol>,
    /// Columns excluding `id` — the ones INSERT writes and UPDATE sets.
    pub non_id_columns: Vec<Symbol>,
    pub insert_sql: String,
    pub update_sql: String,
    pub delete_sql: String,
    pub count_sql: String,
    pub select_by_id_sql: String,
    /// `SELECT <cols> FROM <table>` — used by `Model.all()` emit.
    pub select_all_sql: String,
    /// `SELECT <cols> FROM <table> ORDER BY id DESC LIMIT 1` — used
    /// by `Model.last()` emit.
    pub select_last_sql: String,
    /// `belongs_to` references that the AR layer expects to exist
    /// before save returns true. Rails 5+ default; rendered as a
    /// `Target::find(self.foreign_key).is_none()` short-circuit in
    /// most targets.
    pub belongs_to_checks: Vec<BelongsToCheck>,
    /// `has_many ... dependent: :destroy` — the rows this model's
    /// `destroy` must recursively destroy before DELETEing itself.
    /// Each entry carries the child's table and column list so the
    /// emitter can SELECT children, rehydrate, and call their own
    /// `destroy` (matching Rails callback semantics).
    pub dependent_children: Vec<DependentChild>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BelongsToCheck {
    pub foreign_key: Symbol,
    pub target_class: ClassId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DependentChild {
    pub child_class: ClassId,
    pub child_table: Symbol,
    pub foreign_key_in_child: Symbol,
    pub child_columns: Vec<Symbol>,
    /// `SELECT <cols> FROM <table> WHERE <fk> = ?1` — emitter feeds
    /// this to its DB binding when collecting rows to cascade.
    pub select_by_parent_sql: String,
}

pub fn lower_persistence(model: &Model, app: &App) -> LoweredPersistence {
    let columns: Vec<Symbol> = model.attributes.fields.keys().cloned().collect();
    let non_id_columns: Vec<Symbol> = columns
        .iter()
        .filter(|c| c.as_str() != "id")
        .cloned()
        .collect();

    let table = model.table.0.clone();
    let table_name = table.as_str();

    let insert_cols_list = non_id_columns
        .iter()
        .map(|c| c.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let insert_placeholders = (1..=non_id_columns.len())
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!(
        "INSERT INTO {table_name} ({insert_cols_list}) VALUES ({insert_placeholders})"
    );

    let update_assigns = non_id_columns
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{} = ?{}", c.as_str(), i + 1))
        .collect::<Vec<_>>()
        .join(", ");
    let update_id_placeholder = non_id_columns.len() + 1;
    let update_sql = format!(
        "UPDATE {table_name} SET {update_assigns} WHERE id = ?{update_id_placeholder}"
    );

    let delete_sql = format!("DELETE FROM {table_name} WHERE id = ?1");
    let count_sql = format!("SELECT COUNT(*) FROM {table_name}");

    let all_cols_projection = columns
        .iter()
        .map(|c| c.as_str().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let select_by_id_sql = format!(
        "SELECT {all_cols_projection} FROM {table_name} WHERE id = ?1"
    );
    let select_all_sql = format!("SELECT {all_cols_projection} FROM {table_name}");
    let select_last_sql = format!(
        "SELECT {all_cols_projection} FROM {table_name} ORDER BY id DESC LIMIT 1"
    );

    let belongs_to_checks: Vec<BelongsToCheck> = model
        .associations()
        .filter_map(|a| match a {
            Association::BelongsTo {
                foreign_key,
                target,
                optional: false,
                ..
            } => Some(BelongsToCheck {
                foreign_key: foreign_key.clone(),
                target_class: target.clone(),
            }),
            _ => None,
        })
        .collect();

    let dependent_children: Vec<DependentChild> = model
        .associations()
        .filter_map(|a| match a {
            Association::HasMany {
                target,
                foreign_key,
                dependent: Dependent::Destroy,
                ..
            } => Some((target.clone(), foreign_key.clone())),
            _ => None,
        })
        .map(|(target, fk)| {
            let child_model = app
                .models
                .iter()
                .find(|m| m.name.0 == target.0);
            let child_table = child_model
                .map(|m| m.table.0.clone())
                .unwrap_or_else(|| Symbol::from(crate::naming::pluralize_snake(target.0.as_str())));
            let child_columns: Vec<Symbol> = child_model
                .map(|m| m.attributes.fields.keys().cloned().collect())
                .unwrap_or_default();
            let child_proj = child_columns
                .iter()
                .map(|c| c.as_str().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let mut select_by_parent_sql = String::new();
            write!(
                select_by_parent_sql,
                "SELECT {child_proj} FROM {} WHERE {} = ?1",
                child_table.as_str(),
                fk.as_str(),
            )
            .unwrap();
            DependentChild {
                child_class: target,
                child_table,
                foreign_key_in_child: fk,
                child_columns,
                select_by_parent_sql,
            }
        })
        .collect();

    LoweredPersistence {
        class: model.name.clone(),
        table,
        columns,
        non_id_columns,
        insert_sql,
        update_sql,
        delete_sql,
        count_sql,
        select_by_id_sql,
        select_all_sql,
        select_last_sql,
        belongs_to_checks,
        dependent_children,
    }
}
