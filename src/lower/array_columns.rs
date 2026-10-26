//! PostgreSQL array columns (`t.bigint "tag_ids", array: true`) on a
//! target that stores them (`BuildTarget::stores_array_columns`). Rails
//! reads such a column as an `Array` of its type, nil while a nullable one
//! is unset. The target keeps that Array in the record and stores it in
//! SQLite as JSON text, so the column is typed `Array[<element>]` for the
//! analysis and the lowering both. This runs before the analysis, on the
//! app that target lowers; no other target sees the flag.

use crate::app::App;
use crate::schema::ColumnType;

/// Mark each array column whose element the runtime stores, and retype
/// its attribute on every model over that table.
pub fn store_arrays(app: &mut App) {
    let mut stored = Vec::new();
    for table in app.schema.tables.values_mut() {
        for col in table.columns.iter_mut().filter(|c| c.array && stores_element(&c.col_type)) {
            col.array_stored = true;
            stored.push((table.name.clone(), col.clone()));
        }
    }
    if stored.is_empty() {
        return;
    }
    for model in &mut app.models {
        for (_, col) in stored.iter().filter(|(table, _)| *table == model.table.0) {
            if let Some(ty) = model.attributes.fields.get_mut(&col.name) {
                *ty = crate::ingest::model::ty_of_column_slot(col);
            }
        }
    }
}

/// The element kinds the runtime reads back from JSON as Rails casts
/// them: integers and strings. Other element types keep the element type
/// and the target's `array_column` report.
pub fn stores_element(t: &ColumnType) -> bool {
    matches!(t, ColumnType::Integer | ColumnType::BigInt | ColumnType::String { .. } | ColumnType::Text)
}
