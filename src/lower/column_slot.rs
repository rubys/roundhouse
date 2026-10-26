//! One descriptor for a column whose stored type, public read, and write
//! normalization are not the scalar itself.
//!
//! Temporal columns (`date`, `datetime`, `time`) store ISO-8601 text and
//! expose a native Date or Time. An `interval` column stores a whole-second
//! count as text and exposes `ActiveSupport::Duration`. Schema-less JSON
//! (`t.json`, `t.jsonb`, or `serialize …, coder: JSON`) stores text and
//! exposes the decoded value through `JsonColumn`. Reader, writer,
//! hydration, `[]`, and Arel's formatting ask this slot instead of matching
//! those column types again. Dialect DDL keeps its own type names.
//!
//! Enum label mapping stays outside: it rewrites a scalar slot, it does
//! not change storage. `has_json` stays outside too — a keyed JSON
//! schema is not this codec.

use crate::dialect::Model;
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol};
use crate::schema::{Column, ColumnType};
use crate::span::Span;
use crate::ty::Ty;

use super::typing::with_ty;

/// How a temporal or JSON column crosses the storage boundary.
#[derive(Clone, Copy)]
enum Codec {
    /// `@<col>_raw`. Date and time store ISO text. An interval stores
    /// whole seconds as an integer. Which reads return the logical
    /// value follows `Logical`: `date` and interval `[]` are logical,
    /// timestamps' `[]` is stored text, and only an interval's
    /// `attributes` entry is the cast value.
    Temporal { logical: Logical },
    /// Serialized text in `@<col>`. The public value is untyped.
    Json,
}

#[derive(Clone, Copy)]
enum Logical {
    Date,
    Time,
    Duration,
}

impl Logical {
    fn ty(self) -> Ty {
        match self {
            Logical::Date => Ty::Date,
            Logical::Time => Ty::Time,
            Logical::Duration => Ty::Class {
                id: ClassId(Symbol::from("ActiveSupport::Duration")),
                args: vec![].into(),
            },
        }
    }

    fn parse(self) -> &'static str {
        match self {
            Logical::Date => "parse_db_date",
            Logical::Time => "parse_db_time",
            Logical::Duration => "parse_db_interval",
        }
    }

    fn format(self) -> &'static str {
        match self {
            Logical::Date => "format_db_date",
            Logical::Time => "format_db_time",
            Logical::Duration => "format_db_interval",
        }
    }

    /// `[]` returns the native value for a date and an interval.
    /// Timestamps stay on stored text.
    fn index_reads_logical(self) -> bool {
        matches!(self, Logical::Date | Logical::Duration)
    }

    /// Date and timestamps keep stored text in `attributes`. An
    /// interval's cast value is the duration Rails puts in the hash.
    fn attributes_read_logical(self) -> bool {
        matches!(self, Logical::Duration)
    }

    /// Date and time are ISO text. An interval is a whole-second count,
    /// so SQLite `ORDER BY` follows numeric order.
    fn stored_ty(self) -> Ty {
        match self {
            Logical::Date | Logical::Time => Ty::Str,
            Logical::Duration => Ty::Int,
        }
    }
}

/// Storage, read coercion, and write normalization for one column.
pub struct ColumnSlot<'a> {
    col: &'a Column,
    codec: Option<Codec>,
}

impl<'a> ColumnSlot<'a> {
    /// Classify `col` on `model`. A `has_json` column keeps its keyed
    /// accessors, so a `json`/`jsonb` type there is not this codec.
    pub fn classify(col: &'a Column, model: &Model) -> Self {
        let codec = codec_for(&col.col_type, is_generic_json(col, model));
        Self { col, codec }
    }

    pub fn is_temporal(&self) -> bool {
        matches!(self.codec, Some(Codec::Temporal { .. }))
    }

    pub fn is_json(&self) -> bool {
        matches!(self.codec, Some(Codec::Json))
    }

    pub fn storage_name(&self) -> Symbol {
        storage_name(self.col)
    }

    /// `(logical type, parser, formatter)` for a temporal column.
    pub fn temporal_seam(&self) -> Option<(Ty, &'static str, &'static str)> {
        let Codec::Temporal { logical, .. } = self.codec? else {
            return None;
        };
        Some((logical.ty(), logical.parse(), logical.format()))
    }

    /// Public reader, when it is not the storage ivar. The temporal
    /// return is always nilable: an unset timestamp reads as nil even
    /// on a `null: false` column until a write lands.
    pub fn coerced_reader(&self) -> Option<(Expr, Ty)> {
        match self.codec? {
            Codec::Temporal { logical, .. } => {
                let ret = Ty::Union { variants: vec![logical.ty(), Ty::Nil].into() };
                Some((self.temporal_reader(logical), ret))
            }
            Codec::Json => Some((self.json_load(), Ty::Untyped)),
        }
    }

    /// `[]` / `read_attribute`, when it is not the storage ivar.
    /// Timestamps stay on stored text; `date` and JSON return the
    /// coerced value.
    pub fn index_read(&self) -> Option<Expr> {
        match self.codec? {
            Codec::Temporal { logical } if logical.index_reads_logical() => Some(self.temporal_reader(logical)),
            Codec::Json => Some(self.json_load()),
            Codec::Temporal { .. } => None,
        }
    }

    /// `attributes` decodes JSON. Date and timestamp entries stay stored
    /// text. An interval entry is the duration Rails casts into the hash.
    pub fn attributes_read(&self) -> Option<Expr> {
        match self.codec? {
            Codec::Json => Some(self.json_load()),
            Codec::Temporal { logical } if logical.attributes_read_logical() => {
                Some(self.temporal_reader(logical))
            }
            Codec::Temporal { .. } => None,
        }
    }

    /// Parameter and stored-text types of the public temporal writer.
    pub fn temporal_writer_types(&self) -> Option<(Ty, Ty)> {
        let Codec::Temporal { logical, .. } = self.codec? else {
            return None;
        };
        let stored = logical.stored_ty();
        let logical = logical.ty();
        if self.col.nullable {
            Some((
                Ty::Union { variants: vec![logical, Ty::Nil].into() },
                Ty::Union { variants: vec![stored, Ty::Nil].into() },
            ))
        } else {
            Some((logical, stored))
        }
    }

    /// `ActiveSupport.format_db_*` over `value`, typed `stored_ty`.
    /// The public writer passes its parameter; it is already the
    /// logical type. Constructor, `[]=`, and `update` cast first.
    pub fn format_temporal(&self, value: Expr, stored_ty: Ty) -> Option<Expr> {
        let Codec::Temporal { logical, .. } = self.codec? else {
            return None;
        };
        Some(active_support(value.span, logical.format(), vec![value], stored_ty))
    }

    /// Format a value that is still an untyped bag element. The cast
    /// targets the native type; the stored result is `String` (the
    /// callers that need `String | nil` use [`Self::format_temporal`]).
    pub fn format_cast(&self, value: Expr) -> Option<Expr> {
        let Codec::Temporal { logical, .. } = self.codec? else {
            return None;
        };
        let cast = Expr::new(
            value.span,
            ExprNode::Cast { value, target_ty: logical.ty() },
        );
        Some(active_support(cast.span, logical.format(), vec![cast], logical.stored_ty()))
    }

    /// `JsonColumn.dump`, typed as the string slot. `None` for every
    /// other column, including temporal — those format instead.
    pub fn json_dump(&self, value: Expr) -> Option<Expr> {
        if !self.is_json() {
            return None;
        }
        Some(json_column(value.span, "dump", vec![value], super::model_to_library::ty_of_column_slot(self.col)))
    }

    /// Method hydration calls. JSON skips `dump`. A temporal column
    /// writes `<col>_raw=` even when the model defines `<col>=`. A
    /// scalar whose model defines the writer goes through
    /// `_write_<col>_raw` so the model's method is not the load path.
    /// Enum columns are decided before this and never reach it.
    pub fn hydration_method(&self, model_defines_writer: bool) -> Symbol {
        match self.codec {
            Some(Codec::Json) => json_hydration_method(self.col),
            Some(Codec::Temporal { .. }) => Symbol::from(format!("{}=", self.storage_name().as_str())),
            None if model_defines_writer => {
                Symbol::from(format!("_write_{}_raw", self.col.name.as_str()))
            }
            None => Symbol::from(format!("{}=", self.col.name.as_str())),
        }
    }

    fn temporal_reader(&self, logical: Logical) -> Expr {
        let ivar = with_ty(
            Expr::new(Span::synthetic(), ExprNode::Ivar { name: self.storage_name() }),
            logical.stored_ty(),
        );
        let ret = Ty::Union { variants: vec![logical.ty(), Ty::Nil].into() };
        active_support(Span::synthetic(), logical.parse(), vec![ivar], ret)
    }

    fn json_load(&self) -> Expr {
        let stored = with_ty(
            Expr::new(Span::synthetic(), ExprNode::Ivar { name: self.storage_name() }),
            super::model_to_library::ty_of_column_slot(self.col),
        );
        json_column(Span::synthetic(), "load", vec![stored], Ty::Untyped)
    }
}

/// Hydration writer for a JSON column: stored text, no `JsonColumn.dump`.
pub fn json_hydration_method(col: &Column) -> Symbol {
    Symbol::from(format!("_write_{}_json_raw", col.name.as_str()))
}

/// The ivar a column is stored in. Temporal text lives at `<col>_raw`;
/// every other column, including JSON, uses its own name.
pub fn storage_name(col: &Column) -> Symbol {
    if temporal_codec(&col.col_type).is_some() {
        Symbol::from(format!("{}_raw", col.name.as_str()))
    } else {
        col.name.clone()
    }
}

/// `(logical type, parser, formatter)` from the column type alone.
pub fn temporal_seam(col: &Column) -> Option<(Ty, &'static str, &'static str)> {
    let Codec::Temporal { logical, .. } = temporal_codec(&col.col_type)? else {
        return None;
    };
    Some((logical.ty(), logical.parse(), logical.format()))
}

/// Rewrite an Arel runtime value for a temporal or JSON column.
///
/// A native Date or Time is formatted with the formatter that matches
/// the *value*, so a timestamp column can still carry a Date. A String
/// already holds SQL text and is left alone. JSON is already text at
/// this boundary. `None` means the column is neither, and the caller
/// keeps its own numeric conversion.
pub fn serialize_arel_value(expr: &Expr, column: &ColumnType, nullable: bool) -> Option<Expr> {
    if matches!(column, ColumnType::Json | ColumnType::Jsonb) {
        return Some(expr.clone());
    }
    let Codec::Temporal { logical } = temporal_codec(column)? else {
        return None;
    };
    // Interval text is not yet the stored seconds: `PT1H` and `01:00:00`
    // both have to become `3600`. `format_db_interval` leaves a stored
    // count unchanged. A date or time string is already SQL text.
    if !matches!(logical, Logical::Duration) {
        let method = match expr.ty.as_ref().map(Ty::peel_nilable) {
            Some(Ty::Time) => "format_db_time",
            Some(Ty::Date) => "format_db_date",
            // App RBS signatures may still spell the native type as a root
            // class; nominal namespaced classes are not Time.
            Some(Ty::Class { id, args }) if args.is_empty() && id.0.as_str() == "Time" => "format_db_time",
            Some(Ty::Class { id, args }) if args.is_empty() && id.0.as_str() == "Date" => "format_db_date",
            _ => return Some(expr.clone()),
        };
        let text_ty = if nullable {
            Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() }
        } else {
            Ty::Str
        };
        return Some(active_support(expr.span, method, vec![expr.clone()], text_ty));
    }
    let stored = logical.stored_ty();
    let stored = if nullable {
        Ty::Union { variants: vec![stored, Ty::Nil].into() }
    } else {
        stored
    };
    Some(active_support(expr.span, logical.format(), vec![expr.clone()], stored))
}

/// A temporal column keeps its native seam even if `serialize` also
/// names it. The reader already preferred `parse_db_*`; letting JSON
/// win would drop `<col>=` and the raw reader.
fn codec_for(col_type: &ColumnType, generic_json: bool) -> Option<Codec> {
    if let Some(temporal) = temporal_codec(col_type) {
        Some(temporal)
    } else if generic_json {
        Some(Codec::Json)
    } else {
        None
    }
}

fn temporal_codec(col_type: &ColumnType) -> Option<Codec> {
    match col_type {
        ColumnType::Date => Some(Codec::Temporal { logical: Logical::Date }),
        ColumnType::DateTime | ColumnType::Time => Some(Codec::Temporal { logical: Logical::Time }),
        ColumnType::Interval => Some(Codec::Temporal { logical: Logical::Duration }),
        _ => None,
    }
}

fn is_generic_json(col: &Column, model: &Model) -> bool {
    if super::serialize::json_serialize_columns(model).contains(&col.name) {
        return true;
    }
    matches!(col.col_type, ColumnType::Json | ColumnType::Jsonb)
        && !super::has_json::has_json_decls(&model.body).iter().any(|decl| decl.column == col.name)
}

fn active_support(span: Span, method: &str, args: Vec<Expr>, ty: Ty) -> Expr {
    with_ty(
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        ),
        ty,
    )
}

fn json_column(span: Span, method: &str, args: Vec<Expr>, ty: Ty) -> Expr {
    with_ty(
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("JsonColumn")] })),
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        ),
        ty,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Column;

    fn col(name: &str, col_type: ColumnType) -> Column {
        Column {
            name: Symbol::from(name),
            col_type,
            nullable: true,
            default: None,
            primary_key: false,
            generated: None,
            generated_text_compatible: None,
            generated_int4_compatible: None,
            array: false,
            array_stored: false,
        }
    }

    #[test]
    fn temporal_and_json_share_text_storage_decisions() {
        let reviewed = col("reviewed_at", ColumnType::DateTime);
        let published = col("published_on", ColumnType::Date);
        let metadata = col("metadata", ColumnType::Json);
        let title = col("title", ColumnType::Text);

        assert_eq!(storage_name(&reviewed).as_str(), "reviewed_at_raw");
        assert_eq!(storage_name(&published).as_str(), "published_on_raw");
        assert_eq!(storage_name(&metadata).as_str(), "metadata");

        let (ty, parse, format) = temporal_seam(&reviewed).unwrap();
        assert_eq!(ty, Ty::Time);
        assert_eq!(parse, "parse_db_time");
        assert_eq!(format, "format_db_time");
        let (ty, parse, format) = temporal_seam(&published).unwrap();
        assert_eq!(ty, Ty::Date);
        assert_eq!(parse, "parse_db_date");
        assert_eq!(format, "format_db_date");
        assert!(temporal_seam(&metadata).is_none());
        assert!(temporal_seam(&title).is_none());
    }

    #[test]
    fn date_index_reads_the_native_value_and_timestamps_read_storage() {
        let date = temporal_codec(&ColumnType::Date).unwrap();
        let stamp = temporal_codec(&ColumnType::DateTime).unwrap();
        let time = temporal_codec(&ColumnType::Time).unwrap();
        assert!(matches!(date, Codec::Temporal { logical: Logical::Date }));
        assert!(Logical::Date.index_reads_logical());
        assert!(!Logical::Date.attributes_read_logical());
        assert!(matches!(stamp, Codec::Temporal { logical: Logical::Time }));
        assert!(matches!(time, Codec::Temporal { logical: Logical::Time }));
        assert!(!Logical::Time.index_reads_logical());
        assert!(!Logical::Time.attributes_read_logical());
        let interval = temporal_codec(&ColumnType::Interval).unwrap();
        assert!(matches!(interval, Codec::Temporal { logical: Logical::Duration }));
        assert!(Logical::Duration.index_reads_logical());
        assert!(Logical::Duration.attributes_read_logical());
        assert_eq!(Logical::Duration.stored_ty(), Ty::Int);
    }

    #[test]
    fn a_temporal_column_stays_temporal_when_serialize_also_names_it() {
        let codec = codec_for(&ColumnType::DateTime, true).unwrap();
        assert!(matches!(codec, Codec::Temporal { logical: Logical::Time, .. }));
        assert!(matches!(codec_for(&ColumnType::Text, true), Some(Codec::Json)));
        assert!(codec_for(&ColumnType::Text, false).is_none());
    }
}
