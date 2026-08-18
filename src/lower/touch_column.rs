//! `touch :connected_at` → `self.connected_at = Time.now; touch`.
//!
//! Rails' `touch(*names)` stamps the named columns along with
//! `updated_at`. The shared runtime's `touch` is NO-ARG on purpose:
//! taking the column as a parameter means `self[name] = …`, an index
//! write through a VARIABLE key, and rust2 colors an index-write key
//! for a Hash receiver and for nothing else — the owned `String` lands
//! in `set_index`'s `&str` slot and the app crate fails to compile.
//!
//! At the CALL SITE the column is a literal, so no dynamic key is
//! needed at all: the write becomes an ordinary typed attribute
//! assignment, which every target already emits. Same posture as
//! `insert_all`, `has_json` and `route_format_suffix` — inline what a
//! shared signature could not express portably.
//!
//! Scope: a bare/`self` receiver in a MODEL body (including concerns
//! and association extensions — `for_each_model_body`). An explicit
//! receiver (`other.touch(:col)`) is left alone rather than
//! rewritten: the receiver would be evaluated twice, and nothing in
//! the corpus writes it.
//!
//! STATEMENT POSITIONS ONLY. The rewrite produces two statements, and
//! a `Seq` spliced into an argument position is what took the campfire
//! suite from 47 passing tests to 0 when the Active Storage reader did
//! it. Method bodies, `Seq` elements and `If` branches are statements;
//! anything else keeps its source shape and the honest ArgumentError.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;

pub fn apply_touch_column_lowering(app: &mut App) {
    super::for_each_model_body(app, &mut rewrite_stmt);
}

fn rewrite_stmt(e: &mut Expr) {
    match &mut *e.node {
        ExprNode::Seq { exprs } => {
            for stmt in exprs.iter_mut() {
                rewrite_stmt(stmt);
            }
            return;
        }
        ExprNode::If { then_branch, else_branch, .. } => {
            rewrite_stmt(then_branch);
            rewrite_stmt(else_branch);
            return;
        }
        _ => {}
    }
    let Some(column) = touch_column(e) else { return };
    let span = e.span;
    // `self.<col> = Time.now` — the column writer, not `self[:col]=`:
    // a temporal column's writer is what formats the value for storage
    // (`ActiveSupport.format_db_time`), and it is typed per target.
    let assign = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::SelfRef)),
            method: Symbol::from(format!("{}=", column.as_str())),
            args: vec![Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(Expr::new(
                        span,
                        ExprNode::Const { path: vec![Symbol::from("Time")] },
                    )),
                    method: Symbol::from("now"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            )],
            block: None,
            parenthesized: false,
        },
    );
    // The no-arg `touch` still runs: it is what stamps `updated_at`
    // and issues the UPDATE, so the column write above would otherwise
    // never reach the row.
    let touch = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("touch"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    *e.node = ExprNode::Seq { exprs: vec![assign, touch] };
    e.ty = None;
}

/// The column named by a bare `touch :col` on this model instance.
fn touch_column(e: &Expr) -> Option<Symbol> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else { return None };
    if method.as_str() != "touch" || args.len() != 1 {
        return None;
    }
    match recv {
        None => {}
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => {}
        Some(_) => return None,
    }
    let ExprNode::Lit { value: Literal::Sym { value } } = &*args[0].node else { return None };
    Some(value.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_call(arg: Option<&str>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("touch"),
                args: arg
                    .map(|a| {
                        vec![Expr::new(
                            Span::synthetic(),
                            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(a) } },
                        )]
                    })
                    .unwrap_or_default(),
                block: None,
                parenthesized: false,
            },
        )
    }

    #[test]
    fn rewrites_a_statement_touch_with_a_column() {
        let mut e = touch_call(Some("connected_at"));
        rewrite_stmt(&mut e);
        let ExprNode::Seq { exprs } = &*e.node else { panic!("expected a Seq, got {:?}", e.node) };
        assert_eq!(exprs.len(), 2);
    }

    #[test]
    fn leaves_the_no_arg_form_alone() {
        let mut e = touch_call(None);
        rewrite_stmt(&mut e);
        assert!(matches!(&*e.node, ExprNode::Send { .. }));
    }

    #[test]
    fn leaves_an_explicit_receiver_alone() {
        // `other.touch(:col)` would evaluate the receiver twice.
        let mut e = touch_call(Some("col"));
        if let ExprNode::Send { recv, .. } = &mut *e.node {
            *recv = Some(Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("other") },
            ));
        }
        rewrite_stmt(&mut e);
        assert!(matches!(&*e.node, ExprNode::Send { .. }));
    }
}
