//! Ruby-family read values. Strict targets retain their existing value
//! representation until their nullable runtime and bind capabilities land.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::schema::{ColumnType, Schema};
use crate::ty::Ty;

use super::ir::{ArelOp, Predicate, Value, ValueType};

pub(super) fn normalize(op: &mut ArelOp, schema: &Schema) {
    if let ArelOp::Select(select) = op {
        if let Some(predicate) = &mut select.conditions {
            normalize_predicate(predicate, schema);
        }
    }
}

fn normalize_predicate(predicate: &mut Predicate, schema: &Schema) {
    match predicate {
        Predicate::And(left, right) | Predicate::Or(left, right) => {
            normalize_predicate(left, schema);
            normalize_predicate(right, schema);
        }
        Predicate::Eq(col, Value::Runtime { expr, ty }) => {
            let column = schema.tables.get(&col.table.0)
                .and_then(|table| table.columns.iter().find(|c| c.name == col.column));
            let nullable_column = column.is_some_and(|column| column.nullable && !column.primary_key);
            let nullable_value = expr.ty.as_ref().is_some_and(is_nullable);
            let nullable = nullable_column || nullable_value;
            if let Some(column) = column {
                // A nullable column does not make a scalar RHS optional:
                // Float still needs to_s; only an optional RHS preserves nil.
                *expr = serialized_value_expr(expr, &column.col_type, nullable_value);
            }
            if nullable {
                *ty = match ty {
                    ValueType::Int | ValueType::IntOpt => ValueType::IntOpt,
                    ValueType::Str | ValueType::StrOpt => ValueType::StrOpt,
                    ValueType::Bool | ValueType::BoolOpt => ValueType::BoolOpt,
                    ValueType::FloatOpt => ValueType::FloatOpt,
                };
                if nullable_column {
                    *predicate = Predicate::NullableEq(col.clone(), Value::Runtime { expr: expr.clone(), ty: *ty });
                }
            }
        }
        Predicate::Eq(_, _) | Predicate::NullableEq(_, _) => {}
    }
}

/// Only native temporal values need the writer's formatter. A String ivar
/// (for example a date filter from params) already contains SQL text and
/// must stay a String. Explicit numeric conversion preserves nil; an IR
/// narrowing Cast can be elided after later typing and is not serialization.
fn serialized_value_expr(expr: &Expr, column: &ColumnType, nullable: bool) -> Expr {
    if let Some(serialized) = crate::lower::column_slot::serialize_arel_value(expr, column, nullable) {
        return serialized;
    }
    let text_ty = if nullable { Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() } } else { Ty::Str };
    let converted = match column {
        ColumnType::Float | ColumnType::Decimal { .. } => {
            let text = Expr::new(expr.span, ExprNode::Send {
                recv: Some(expr.clone()), method: Symbol::from("to_s"), args: vec![],
                block: None, parenthesized: false,
            });
            if nullable {
                // The builder admits only pure ivar reads here. Keep this
                // semantic conversion explicit through subsequent typing.
                Expr::new(expr.span, ExprNode::If {
                    cond: Expr::new(expr.span, ExprNode::Send {
                        recv: Some(expr.clone()), method: Symbol::from("nil?"), args: vec![],
                        block: None, parenthesized: false,
                    }),
                    then_branch: Expr::new(expr.span, ExprNode::Lit { value: Literal::Nil }),
                    else_branch: text,
                })
            } else {
                text
            }
        }
        _ => return expr.clone(),
    };
    crate::lower::typing::with_ty(converted, text_ty)
}

fn is_nullable(ty: &Ty) -> bool {
    match ty {
        Ty::Nil => true,
        Ty::Union { variants } => variants.iter().any(is_nullable),
        _ => false,
    }
}
