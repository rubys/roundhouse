//! `recv.each.with_index(offset?) { |item, index| … }` → `each_with_index`.
//!
//! Enumerator chaining (`each.with_index`) is a Spinel AOT subset gap when
//! the block body makes a keyword call that closes over the index — Writebook's
//! `Positionable#move_to_position` fails with:
//!
//! ```text
//! a String is not yet shared by reference through an Array's chained index
//! into an appending block
//! ```
//!
//! `Array#each_with_index` compiles and means the same thing when the offset
//! is zero / omitted. A non-zero offset becomes `index = __with_index_i + offset`
//! at the top of the block so the caller's binding keeps its name and value.
//!
//! Shared home (not a Spinel-only emit patch): every target gets the flatter
//! call, and CRuby/`each_with_index` semantics match `each.with_index`.
//! `map.with_index` is left alone — Spinel already accepts that shape.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal, LValue};
use crate::ident::{Symbol, VarId};
use crate::ty::Ty;

pub fn apply_each_with_index_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let span = expr.span;
    let (collection, offset, mut block) = {
        let ExprNode::Send {
            recv: Some(each_expr),
            method,
            args,
            block,
            ..
        } = &mut *expr.node
        else {
            return;
        };
        if method.as_str() != "with_index" || args.len() > 1 || block.is_none() {
            return;
        }
        let ExprNode::Send {
            recv: collection,
            method: each_method,
            args: each_args,
            block: each_block,
            ..
        } = &*each_expr.node
        else {
            return;
        };
        if each_method.as_str() != "each" || !each_args.is_empty() || each_block.is_some() {
            return;
        }

        // Only literal Int offsets are safe to materialize into the
        // loop (`index = __with_index_i + offset`). A dynamic offset
        // must evaluate once before iteration — cloning it into the
        // body would re-run side effects and see reassigned locals.
        let offset = match args.as_slice() {
            [] => None,
            [arg] => match &*arg.node {
                ExprNode::Lit {
                    value: Literal::Int { value: 0 },
                } => None,
                ExprNode::Lit {
                    value: Literal::Int { .. },
                } => Some(arg.clone()),
                _ => return,
            },
            _ => return,
        };

        (collection.clone(), offset, block.take().expect("checked above"))
    };

    if let Some(offset) = offset {
        inject_offset_binding(&mut block, offset, span);
    }

    *expr.node = ExprNode::Send {
        recv: collection,
        method: Symbol::from("each_with_index"),
        args: Vec::new(),
        block: Some(block),
        parenthesized: false,
    };
}

/// Rename the index parameter to `__with_index_i` and prepend
/// `index = __with_index_i + offset` so the block body keeps reading
/// the caller's name at the offset-adjusted value.
fn inject_offset_binding(block: &mut Expr, offset: Expr, span: crate::span::Span) {
    let ExprNode::Lambda { params, body, .. } = &mut *block.node else {
        return;
    };
    if params.len() < 2 {
        return;
    }
    let index_name = params[1].clone();
    let tmp = Symbol::from("__with_index_i");
    params[1] = tmp.clone();

    let mut tmp_var = Expr::new(
        span,
        ExprNode::Var {
            id: VarId(0),
            name: tmp,
        },
    );
    tmp_var.ty = Some(Ty::Int);

    let mut sum = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(tmp_var),
            method: Symbol::from("+"),
            args: vec![offset],
            block: None,
            parenthesized: false,
        },
    );
    sum.ty = Some(Ty::Int);

    let assign = Expr::new(
        span,
        ExprNode::Assign {
            target: LValue::Var {
                id: VarId(0),
                name: index_name,
            },
            value: sum,
        },
    );

    match &mut *body.node {
        ExprNode::Seq { exprs } => {
            exprs.insert(0, assign);
        }
        _ => {
            let old = std::mem::replace(
                body,
                Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
            );
            *body = Expr::new(
                span,
                ExprNode::Seq {
                    exprs: vec![assign, old],
                },
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn lit_int(n: i64) -> Expr {
        let mut e = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value: n } });
        e.ty = Some(Ty::Int);
        e
    }

    fn var(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var {
                id: VarId(0),
                name: Symbol::from(name),
            },
        )
    }

    fn lambda(params: &[&str], body: Expr) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                params: params.iter().map(|p| Symbol::from(*p)).collect(),
                rest_param: None,
                block_param: None,
                body,
                block_style: Default::default(),
            },
        )
    }

    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>, block: Option<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv,
                method: Symbol::from(method),
                args,
                block,
                parenthesized: false,
            },
        )
    }

    #[test]
    fn collapses_each_with_index_without_offset() {
        let each = send(Some(var("items")), "each", vec![], None);
        let mut expr = send(
            Some(each),
            "with_index",
            vec![],
            Some(lambda(&["item", "index"], lit_int(0))),
        );
        rewrite(&mut expr);
        let ExprNode::Send { method, args, block, recv, .. } = &*expr.node else {
            panic!("expected Send");
        };
        assert_eq!(method.as_str(), "each_with_index");
        assert!(args.is_empty());
        assert!(recv.is_some());
        let ExprNode::Lambda { params, .. } = &*block.as_ref().unwrap().node else {
            panic!("expected Lambda");
        };
        assert_eq!(params[1].as_str(), "index");
    }

    #[test]
    fn injects_offset_binding_for_nonzero_start() {
        let each = send(Some(var("items")), "each", vec![], None);
        let mut expr = send(
            Some(each),
            "with_index",
            vec![lit_int(1)],
            Some(lambda(&["item", "index"], lit_int(0))),
        );
        rewrite(&mut expr);
        let ExprNode::Send { method, block, .. } = &*expr.node else {
            panic!("expected Send");
        };
        assert_eq!(method.as_str(), "each_with_index");
        let ExprNode::Lambda { params, body, .. } = &*block.as_ref().unwrap().node else {
            panic!("expected Lambda");
        };
        assert_eq!(params[1].as_str(), "__with_index_i");
        let ExprNode::Seq { exprs } = &*body.node else {
            panic!("expected Seq, got {}", body.node.kind_str());
        };
        assert!(matches!(&*exprs[0].node, ExprNode::Assign { .. }));
    }

    #[test]
    fn leaves_map_with_index_alone() {
        let map = send(Some(var("items")), "map", vec![], None);
        let mut expr = send(
            Some(map),
            "with_index",
            vec![lit_int(1)],
            Some(lambda(&["item", "index"], lit_int(0))),
        );
        rewrite(&mut expr);
        let ExprNode::Send { method, .. } = &*expr.node else {
            panic!("expected Send");
        };
        assert_eq!(method.as_str(), "with_index");
    }
}
