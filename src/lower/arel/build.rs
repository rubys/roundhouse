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
use crate::lower::model_associations::{AssocKind, AssociationEdge};
use crate::schema::{ColumnType, Schema, Table};

use super::ir::{
    ArelOp, ColumnSpec, LimitSpec, Predicate, PreloadDirective, Select, Value, ValueType,
};

/// Try to build an `ArelOp` from a Send call site. Returns
/// `Some((op, owner))` when the pattern matches, `None` otherwise —
/// callers leave the original Send in place when None.
///
/// `owner` is the model class the Send dispatches against; the
/// caller (lowerer pass) hands it to the visitor so Select knows
/// which model to hydrate.
///
/// Recognizes both base-shape Sends (`Model.where(...)`,
/// `Model.find_by(...)`, `Model.all`, `Model.count`,
/// `Model.exists?(...)`) and chain-shape Sends with trailing
/// `.order(col: :dir)` / `.limit(n)` / `.includes(:assoc)`
/// modifiers. The recursive arm walks the chain bottom-up: the
/// innermost recv must resolve to a base-shape Send for the chain
/// to lift to Arel.
pub fn try_build_arel(
    send: &Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Option<(ArelOp, ClassId)> {
    try_build_arel_with_assocs(send, schema, registry, &[])
}

/// As `try_build_arel`, but with the app's association graph so
/// `includes(:assoc)` lifts to a `PreloadDirective` instead of being
/// dropped. Callers without the graph (model bodies, tests) use the
/// 3-arg wrapper and get the legacy drop-includes behavior.
pub fn try_build_arel_with_assocs(
    send: &Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[AssociationEdge],
) -> Option<(ArelOp, ClassId)> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = send.node.as_ref() else {
        return None;
    };

    // Chain-modifier arm — recurse into recv, layer this method on top.
    // `try_chain_recv` lets a bare Const recv act as an implicit
    // `Const.all` so `Article.order(...)` (Rails-style) and
    // `Article.includes(:c).order(...)` both lift cleanly.
    match method.as_str() {
        "order" => {
            let (op, owner) = try_chain_recv(recv, schema, registry, assocs)?;
            return apply_order(op, args).map(|op| (op, owner));
        }
        "limit" => {
            let (op, owner) = try_chain_recv(recv, schema, registry, assocs)?;
            return apply_limit(op, args).map(|op| (op, owner));
        }
        // `includes(:assoc)` — capture each association as a
        // `PreloadDirective` on the underlying Select so the visitor
        // can batch-load it (issue #27). When the association graph
        // wasn't supplied (empty `assocs`) or the assoc doesn't
        // resolve to a has_many edge, it silently falls back to the
        // legacy no-op drop: recurse and keep the rest of the chain.
        "includes" | "preload" | "eager_load" => {
            let (op, owner) = try_chain_recv(recv, schema, registry, assocs)?;
            let op = attach_preloads(op, &owner, args, registry, assocs);
            return Some((op, owner));
        }
        _ => {}
    }

    // Base arm — recv must be a Const that resolves to a model in the
    // registry; method must be one of the base AR shapes we recognize.
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

/// Treat the recv of a chain modifier as a recognizable Arel base.
/// Two shapes count:
///   - A Send `try_build_arel` already recognizes (`Model.where(...)`,
///     another chain link).
///   - A bare `Const` that resolves to a known model — implicit
///     `.all` shape (Rails-style: `Article.order(...)` is the same
///     as `Article.all.order(...)`).
fn try_chain_recv(
    recv: &Expr,
    schema: &Schema,
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[AssociationEdge],
) -> Option<(ArelOp, ClassId)> {
    if let Some((op, owner)) = try_build_arel_with_assocs(recv, schema, registry, assocs) {
        return Some((op, owner));
    }
    // Implicit `.all` for bare-Const recv. `Article.order(...)` →
    // `Article.all.order(...)`.
    let class_id = const_to_class_id(recv, registry)?;
    let info = registry.get(&class_id)?;
    let table_ref = info.table.as_ref()?.clone();
    let _ = schema.tables.get(&table_ref.0)?;
    Some((ArelOp::Select(build_all(&table_ref)), class_id))
}

/// Resolve each `:assoc` symbol arg of an `includes(...)` call against
/// the association graph and attach a `PreloadDirective` to the
/// underlying Select. Only `has_many` edges are eager-loaded for now
/// (`belongs_to` / `has_one` / `:through` are out of scope, issue #27);
/// unresolved args are skipped, preserving the legacy no-op behavior.
fn attach_preloads(
    op: ArelOp,
    owner: &ClassId,
    args: &[Expr],
    registry: &HashMap<ClassId, ClassInfo>,
    assocs: &[AssociationEdge],
) -> ArelOp {
    let ArelOp::Select(mut sel) = op else {
        return op;
    };
    for arg in args {
        let ExprNode::Lit { value: Literal::Sym { value: assoc_name } } = arg.node.as_ref() else {
            continue;
        };
        let Some(edge) = assocs.iter().find(|e| {
            &e.from == owner && &e.name == assoc_name && e.kind == AssocKind::HasMany
        }) else {
            continue;
        };
        let Some(table_ref) = registry.get(&edge.to).and_then(|i| i.table.clone()) else {
            continue;
        };
        sel.preloads.push(PreloadDirective {
            name: assoc_name.clone(),
            target_class: edge.to.clone(),
            target_table: table_ref,
            foreign_key: edge.foreign_key.clone(),
        });
    }
    ArelOp::Select(sel)
}

// ---------------------------------------------------------------------------
// Chain modifiers — extend an existing ArelOp with order / limit
// ---------------------------------------------------------------------------

/// `.order(col: :dir, …)` — extract kwargs, append Order entries to
/// the inner Select. Only Select takes orders; layering on
/// Insert/Update/Delete returns None (silly but defensive).
fn apply_order(op: ArelOp, args: &[Expr]) -> Option<ArelOp> {
    let mut sel = match op {
        ArelOp::Select(s) => s,
        _ => return None,
    };
    let entries = single_kwargs_hash(args)?;
    for (k, v) in entries {
        let col_name = key_as_column_symbol(k)?;
        let direction = direction_from_value(v)?;
        sel.orders.push(super::ir::Order {
            column: super::ir::ColRef {
                table: sel.table.clone(),
                column: col_name,
            },
            direction,
        });
    }
    Some(ArelOp::Select(sel))
}

/// `.limit(n)` — single positional integer literal becomes the
/// LimitSpec. Existing limits are overwritten (Rails semantics).
fn apply_limit(op: ArelOp, args: &[Expr]) -> Option<ArelOp> {
    let mut sel = match op {
        ArelOp::Select(s) => s,
        _ => return None,
    };
    if args.len() != 1 {
        return None;
    }
    let ExprNode::Lit { value: Literal::Int { value } } = args[0].node.as_ref() else {
        return None;
    };
    if *value < 0 {
        return None;
    }
    sel.limit = Some(super::ir::LimitSpec(*value as u64));
    Some(ArelOp::Select(sel))
}

/// Common destructure for `.order(col: :dir, …)` /
/// `.where(col: val, …)` — args must be exactly one kwargs hash.
fn single_kwargs_hash(args: &[Expr]) -> Option<&Vec<(Expr, Expr)>> {
    if args.len() != 1 {
        return None;
    }
    let ExprNode::Hash { entries, kwargs } = args[0].node.as_ref() else {
        return None;
    };
    if !*kwargs || entries.is_empty() {
        return None;
    }
    Some(entries)
}

/// `.order(col: :asc)` direction value is a Sym literal — anything
/// else (interpolation, dynamic) falls out to runtime fallback.
fn direction_from_value(v: &Expr) -> Option<super::ir::Direction> {
    match v.node.as_ref() {
        ExprNode::Lit { value: Literal::Sym { value } } => match value.as_str() {
            "asc" | "ASC" => Some(super::ir::Direction::Asc),
            "desc" | "DESC" => Some(super::ir::Direction::Desc),
            _ => None,
        },
        _ => None,
    }
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
                preloads: vec![],
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
                preloads: vec![],
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
                preloads: vec![],
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
                preloads: vec![],
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
                preloads: vec![],
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
    fn recognizes_all_then_order_chain() {
        // Comment.all.order(article_id: :desc)
        let (schema, registry) = fixture();
        let inner = make_send(const_recv("Comment"), "all", vec![]);
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(
                    lit_sym("article_id"),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Sym { value: Symbol::from("desc") } },
                    ),
                )],
                kwargs: true,
            },
        );
        let send = make_send(inner, "order", vec![kwargs]);
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                assert!(matches!(s.columns, ColumnSpec::All));
                assert_eq!(s.orders.len(), 1);
                assert_eq!(s.orders[0].column.column.as_str(), "article_id");
                assert!(matches!(s.orders[0].direction, super::super::ir::Direction::Desc));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn recognizes_includes_then_order_chain() {
        // Comment.includes(:article).order(article_id: :asc) — includes
        // is a no-op chain link; the recognizer drops it and proceeds.
        let (schema, registry) = fixture();
        let includes = make_send(
            const_recv("Comment"),
            "includes",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Sym { value: Symbol::from("article") } },
            )],
        );
        // .includes leg alone: returns None (not a recognizable base on its own
        // because it has no `Comment.<base>` underneath when called bare).
        // To recognize, the chain needs a base — wrap in `.all` first.
        let inner = make_send(const_recv("Comment"), "all", vec![]);
        let with_includes = make_send(inner, "includes", vec![Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from("article") } },
        )]);
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(
                    lit_sym("article_id"),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Sym { value: Symbol::from("asc") } },
                    ),
                )],
                kwargs: true,
            },
        );
        let send = make_send(with_includes, "order", vec![kwargs]);
        let _ = includes; // raw `Comment.includes(...)` not exercised here
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                assert!(matches!(s.orders[0].direction, super::super::ir::Direction::Asc));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn recognizes_all_then_limit_chain() {
        // Comment.all.limit(5)
        let (schema, registry) = fixture();
        let inner = make_send(const_recv("Comment"), "all", vec![]);
        let send = make_send(
            inner,
            "limit",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: 5 } },
            )],
        );
        let (op, _) = try_build_arel(&send, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                assert_eq!(s.limit, Some(super::super::ir::LimitSpec(5)));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn recognizes_where_then_order_then_limit_chain() {
        // Comment.where(article_id: 1).order(id: :desc).limit(10)
        let (schema, registry) = fixture();
        let where_kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(
                    lit_sym("article_id"),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Int { value: 1 } },
                    ),
                )],
                kwargs: true,
            },
        );
        let where_call = make_send(const_recv("Comment"), "where", vec![where_kwargs]);
        let order_kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(
                    lit_sym("id"),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Lit { value: Literal::Sym { value: Symbol::from("desc") } },
                    ),
                )],
                kwargs: true,
            },
        );
        let order_call = make_send(where_call, "order", vec![order_kwargs]);
        let limit_call = make_send(
            order_call,
            "limit",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: 10 } },
            )],
        );
        let (op, _) = try_build_arel(&limit_call, &schema, &registry).expect("should match");
        match op {
            ArelOp::Select(s) => {
                assert!(s.conditions.is_some(), "where preserved");
                assert_eq!(s.orders.len(), 1, "order preserved");
                assert_eq!(s.limit, Some(super::super::ir::LimitSpec(10)));
            }
            _ => panic!("expected Select"),
        }
    }

    #[test]
    fn does_not_recognize_order_with_dynamic_direction() {
        // Comment.all.order(article_id: var) — runtime variable as direction
        let (schema, registry) = fixture();
        let inner = make_send(const_recv("Comment"), "all", vec![]);
        let kwargs = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![(
                    lit_sym("article_id"),
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("dir") },
                    ),
                )],
                kwargs: true,
            },
        );
        let send = make_send(inner, "order", vec![kwargs]);
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
