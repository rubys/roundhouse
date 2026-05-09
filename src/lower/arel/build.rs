//! `try_build_arel` — recognize call sites whose Arel tree the lowerer
//! can build entirely at transpile time.
//!
//! Returns `None` when the receiver, method, or args don't match a
//! statically-resolvable shape. None routes to runtime fallback in
//! Phase 2 (runtime Arel module in framework Ruby + per-target visitor).
//!
//! Phase 1 priorities (in order of validation impact):
//!
//! 1. `<Model>.where(<col>: <value>, …)` — the has_many proxy body
//!    (`Comment.where(article_id: @id)`). Without this, has_many
//!    readers call into a nil adapter.
//! 2. `<Model>.find_by(<col>: <value>, …)` — the belongs_to body
//!    (`Article.find_by(id: @article_id)`).
//! 3. `<Model>.all` — full table scan.
//! 4. `<Model>.count` — row count.
//! 5. `<Model>.exists?(<col>: <value>, …)` — boolean lookup.
//!
//! `<Model>.find(id)` is intentionally NOT recognized: the public
//! `Base#find` in framework Ruby already delegates to
//! `_adapter_find_by_id`, and that delegation chain has well-defined
//! raise-on-missing semantics that an inline rewrite would shortcut.
//! When a controller writes `Article.find(id)`, it stays a Send into
//! framework Ruby; only the framework's own internal AR plumbing
//! reaches the per-model `_adapter_*` primitives.

use std::collections::HashMap;

use crate::analyze::ClassInfo;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol, TableRef};
use crate::schema::{ColumnType, Schema, Table};

use super::ir::{
    ArelOp, ColumnSpec, LimitSpec, Predicate, Select, Value, ValueType,
};

/// Try to build an `ArelOp` from a Send call site. Returns
/// `Some((op, owner))` when the pattern matches, `None` otherwise —
/// callers leave the original Send in place when None.
///
/// `owner` is the model class the Send dispatches against; the
/// caller (lowerer pass) hands it to the visitor so Select knows
/// which model to hydrate.
pub fn try_build_arel(
    send: &Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Option<(ArelOp, ClassId)> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = send.node.as_ref() else {
        return None;
    };
    let class_id = const_to_class_id(recv, registry)?;
    let info = registry.get(&class_id)?;
    let table_ref = info.table.as_ref()?.clone();
    let table = schema.tables.get(&table_ref.0)?;

    let op = match method.as_str() {
        "all" => Some(ArelOp::Select(build_all(&table_ref))),
        "count" => Some(ArelOp::Select(build_count(&table_ref))),
        "where" => build_where_kwargs(args, table, &table_ref).map(ArelOp::Select),
        "find_by" => build_find_by_kwargs(args, table, &table_ref).map(ArelOp::Select),
        "exists?" => build_exists_kwargs(args, table, &table_ref).map(ArelOp::Select),
        _ => None,
    }?;
    Some((op, class_id))
}

// ---------------------------------------------------------------------------
// Receiver resolution
// ---------------------------------------------------------------------------

/// Resolve a `Const` Expr to a registry-known `ClassId`. Tries the
/// joined path verbatim first (`Foo::Bar` → `ClassId("Foo::Bar")`),
/// then the last segment alone (`Bar`) since the model lowerer
/// registers app classes under their bare name. Returns None when
/// the receiver isn't a Const or the class isn't registered.
fn const_to_class_id(recv: &Expr, registry: &HashMap<ClassId, ClassInfo>) -> Option<ClassId> {
    let ExprNode::Const { path } = recv.node.as_ref() else {
        return None;
    };
    if path.is_empty() {
        return None;
    }
    let joined = ClassId(Symbol::from(
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
    ));
    if registry.contains_key(&joined) {
        return Some(joined);
    }
    let last = ClassId(path.last().unwrap().clone());
    if registry.contains_key(&last) {
        return Some(last);
    }
    None
}

// ---------------------------------------------------------------------------
// Per-method builders
// ---------------------------------------------------------------------------

fn build_all(table_ref: &TableRef) -> Select {
    Select {
        table: table_ref.clone(),
        columns: ColumnSpec::All,
        conditions: None,
        orders: vec![],
        limit: None,
        joins: vec![],
    }
}

fn build_count(table_ref: &TableRef) -> Select {
    Select {
        table: table_ref.clone(),
        columns: ColumnSpec::Count,
        conditions: None,
        orders: vec![],
        limit: None,
        joins: vec![],
    }
}

/// `Model.where(col1: v1, col2: v2)` → AND-chained Eq predicates.
/// Returns None if the args aren't a single trailing kwargs hash or
/// any key isn't a literal column symbol present in the schema.
fn build_where_kwargs(args: &[Expr], table: &Table, table_ref: &TableRef) -> Option<Select> {
    let preds = predicates_from_kwargs(args, table, table_ref)?;
    Some(Select {
        table: table_ref.clone(),
        columns: ColumnSpec::All,
        conditions: Some(preds),
        orders: vec![],
        limit: None,
        joins: vec![],
    })
}

/// `Model.find_by(col1: v1, …)` → AND-chained Eq + `LIMIT 1` →
/// nilable single hydrate at the visitor.
fn build_find_by_kwargs(args: &[Expr], table: &Table, table_ref: &TableRef) -> Option<Select> {
    let preds = predicates_from_kwargs(args, table, table_ref)?;
    Some(Select {
        table: table_ref.clone(),
        columns: ColumnSpec::All,
        conditions: Some(preds),
        orders: vec![],
        limit: Some(LimitSpec(1)),
        joins: vec![],
    })
}

/// `Model.exists?(col1: v1, …)` → AND-chained Eq + `LIMIT 1` over
/// `SELECT 1` projection → bool from `step?` at the visitor.
fn build_exists_kwargs(args: &[Expr], table: &Table, table_ref: &TableRef) -> Option<Select> {
    let preds = predicates_from_kwargs(args, table, table_ref)?;
    Some(Select {
        table: table_ref.clone(),
        columns: ColumnSpec::Exists,
        conditions: Some(preds),
        orders: vec![],
        limit: Some(LimitSpec(1)),
        joins: vec![],
    })
}

// ---------------------------------------------------------------------------
// Kwargs → AND-chained Eq
// ---------------------------------------------------------------------------

/// Extract the trailing kwargs hash from a Send's args, build an
/// `Eq(col, value)` per entry, AND-chain them, return the resulting
/// Predicate. Returns None if:
///   - args isn't exactly one Hash with `kwargs: true`
///   - the hash is empty (where() with no args isn't a useful Arel)
///   - any key isn't a Sym or doesn't match a schema column
fn predicates_from_kwargs(
    args: &[Expr],
    table: &Table,
    table_ref: &TableRef,
) -> Option<Predicate> {
    if args.len() != 1 {
        return None;
    }
    let ExprNode::Hash { entries, kwargs } = args[0].node.as_ref() else {
        return None;
    };
    if !*kwargs || entries.is_empty() {
        return None;
    }

    let mut acc: Option<Predicate> = None;
    for (k, v) in entries {
        let col_name = key_as_column_symbol(k)?;
        let col = table
            .columns
            .iter()
            .find(|c| c.name.as_str() == col_name.as_str())?;
        let pred = Predicate::Eq(
            super::ir::ColRef { table: table_ref.clone(), column: col_name },
            value_from_expr(v, &col.col_type),
        );
        acc = Some(match acc {
            None => pred,
            Some(prev) => Predicate::And(Box::new(prev), Box::new(pred)),
        });
    }
    acc
}

/// Extract the column-symbol from a hash entry's key. Accepts only
/// `Lit::Sym` — anything else (interpolation, dynamic) routes the
/// pattern to runtime fallback.
fn key_as_column_symbol(key: &Expr) -> Option<Symbol> {
    match key.node.as_ref() {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        _ => None,
    }
}

/// Map a value expression to an Arel `Value`. Literal exprs become
/// `Literal*` variants (baked into SQL); any other expression
/// becomes `Runtime { expr, ty }` with `ty` inferred from the
/// schema column's declared type.
fn value_from_expr(expr: &Expr, col_type: &ColumnType) -> Value {
    match expr.node.as_ref() {
        ExprNode::Lit { value: Literal::Int { value } } => Value::LiteralInt(*value),
        ExprNode::Lit { value: Literal::Str { value } } => Value::LiteralStr(value.clone()),
        ExprNode::Lit { value: Literal::Bool { value } } => Value::LiteralBool(*value),
        ExprNode::Lit { value: Literal::Nil } => Value::LiteralNull,
        _ => Value::Runtime {
            expr: expr.clone(),
            ty: value_type_for(col_type),
        },
    }
}

fn value_type_for(col_type: &ColumnType) -> ValueType {
    match col_type {
        ColumnType::Integer
        | ColumnType::BigInt
        | ColumnType::Reference { .. } => ValueType::Int,
        ColumnType::Boolean => ValueType::Bool,
        _ => ValueType::Str,
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::ClassInfo;
    use crate::expr::{ExprNode, Literal};
    use crate::ident::TableRef;
    use crate::schema::{Column, Schema, Table};
    use crate::span::Span;
    use indexmap::IndexMap;

    fn fixture() -> (Schema, HashMap<ClassId, ClassInfo>) {
        let mut tables = IndexMap::new();
        tables.insert(
            Symbol::from("comments"),
            Table {
                name: Symbol::from("comments"),
                columns: vec![
                    Column {
                        name: Symbol::from("id"),
                        col_type: ColumnType::Integer,
                        nullable: false,
                        default: None,
                        primary_key: true,
                    },
                    Column {
                        name: Symbol::from("article_id"),
                        col_type: ColumnType::Integer,
                        nullable: false,
                        default: None,
                        primary_key: false,
                    },
                    Column {
                        name: Symbol::from("body"),
                        col_type: ColumnType::Text,
                        nullable: false,
                        default: None,
                        primary_key: false,
                    },
                ],
                indexes: vec![],
                foreign_keys: vec![],
            },
        );
        let schema = Schema { tables };

        let mut registry = HashMap::new();
        let mut comment_info = ClassInfo::default();
        comment_info.table = Some(TableRef(Symbol::from("comments")));
        registry.insert(ClassId(Symbol::from("Comment")), comment_info);
        (schema, registry)
    }

    fn const_recv(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from(name)] },
        )
    }

    fn ivar(name: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from(name) })
    }

    fn lit_sym(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } },
        )
    }

    fn make_send(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        )
    }

    #[test]
    fn recognizes_has_many_proxy_where() {
        // Comment.where(article_id: @id)
        let (schema, registry) = fixture();
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(lit_sym("article_id"), ivar("id"))],
                kwargs: true,
            },
        );
        let send = make_send(const_recv("Comment"), "where", vec![kwargs]);
        let (op, owner) = try_build_arel(&send, &schema, &registry).expect("should match");
        assert_eq!(owner.0.as_str(), "Comment");
        match op {
            ArelOp::Select(s) => {
                assert!(matches!(s.columns, ColumnSpec::All));
                assert!(s.limit.is_none());
                let Some(Predicate::Eq(col, val)) = s.conditions else {
                    panic!("expected single Eq predicate");
                };
                assert_eq!(col.column.as_str(), "article_id");
                let Value::Runtime { ty, .. } = val else {
                    panic!("expected runtime value");
                };
                assert_eq!(ty, ValueType::Int);
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn recognizes_belongs_to_find_by() {
        // Comment.find_by(id: @article_id)
        let (schema, registry) = fixture();
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(lit_sym("id"), ivar("article_id"))],
                kwargs: true,
            },
        );
        let send = make_send(const_recv("Comment"), "find_by", vec![kwargs]);
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                assert!(matches!(s.columns, ColumnSpec::All));
                assert_eq!(s.limit, Some(LimitSpec(1)));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn recognizes_all() {
        let (schema, registry) = fixture();
        let send = make_send(const_recv("Comment"), "all", vec![]);
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                assert!(matches!(s.columns, ColumnSpec::All));
                assert!(s.conditions.is_none());
                assert!(s.limit.is_none());
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn recognizes_count() {
        let (schema, registry) = fixture();
        let send = make_send(const_recv("Comment"), "count", vec![]);
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => assert!(matches!(s.columns, ColumnSpec::Count)),
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn recognizes_exists_with_id() {
        let (schema, registry) = fixture();
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(lit_sym("id"), ivar("article_id"))],
                kwargs: true,
            },
        );
        let send = make_send(const_recv("Comment"), "exists?", vec![kwargs]);
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                assert!(matches!(s.columns, ColumnSpec::Exists));
                assert_eq!(s.limit, Some(LimitSpec(1)));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn does_not_recognize_unknown_class() {
        let (schema, registry) = fixture();
        let send = make_send(const_recv("Widget"), "all", vec![]);
        assert!(try_build_arel(&send, &schema, &registry).is_none());
    }

    #[test]
    fn does_not_recognize_unknown_method() {
        let (schema, registry) = fixture();
        let send = make_send(const_recv("Comment"), "create!", vec![]);
        assert!(try_build_arel(&send, &schema, &registry).is_none());
    }

    #[test]
    fn does_not_recognize_where_with_unknown_column() {
        let (schema, registry) = fixture();
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(lit_sym("nonexistent"), ivar("id"))],
                kwargs: true,
            },
        );
        let send = make_send(const_recv("Comment"), "where", vec![kwargs]);
        assert!(try_build_arel(&send, &schema, &registry).is_none());
    }

    #[test]
    fn does_not_recognize_where_without_kwargs() {
        let (schema, registry) = fixture();
        // Comment.where("article_id = ?", 1) — string-and-args, runtime-only
        let send = make_send(
            const_recv("Comment"),
            "where",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit {
                    value: Literal::Str { value: "article_id = ?".into() },
                },
            )],
        );
        assert!(try_build_arel(&send, &schema, &registry).is_none());
    }

    #[test]
    fn does_not_recognize_find_intentionally() {
        // `Model.find(id)` is intentionally NOT recognized — see
        // module docs. The framework Ruby `Base#find` chain handles it.
        let (schema, registry) = fixture();
        let send = make_send(
            const_recv("Comment"),
            "find",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: 1 } },
            )],
        );
        assert!(try_build_arel(&send, &schema, &registry).is_none());
    }

    #[test]
    fn recognizes_multi_kwarg_where_as_and_chain() {
        // Comment.where(article_id: @id, body: "x") → AND of two Eqs
        let (schema, registry) = fixture();
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![
                    (lit_sym("article_id"), ivar("id")),
                    (
                        lit_sym("body"),
                        Expr::new(
                            Span::synthetic(),
                            ExprNode::Lit { value: Literal::Str { value: "x".into() } },
                        ),
                    ),
                ],
                kwargs: true,
            },
        );
        let send = make_send(const_recv("Comment"), "where", vec![kwargs]);
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                let Some(Predicate::And(_, _)) = s.conditions else {
                    panic!("expected And-chained predicate");
                };
            }
            _ => panic!("expected Select"),
        }
    }
}
