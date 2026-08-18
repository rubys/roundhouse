//! Rails' record methods that take a COLUMN NAME, rewritten at the call
//! site where that name is a literal:
//!
//!   touch :connected_at            -> self.connected_at = Time.now; touch
//!   increment!(:connections, touch: true)
//!                                  -> self.connections = self.connections + 1; touch
//!   decrement!(:connections, touch: true)
//!                                  -> self.connections = self.connections - 1; touch
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

pub fn apply_column_ops_lowering(app: &mut App) {
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
    let Some(op) = column_op(e) else { return };
    let span = e.span;
    let column = op.column;
    // The column WRITER, not `self[:col]=`: a temporal column's writer
    // is what formats the value for storage
    // (`ActiveSupport.format_db_time`), and every writer is typed per
    // target.
    let value = match op.kind {
        OpKind::Touch => Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("Time")] })),
                method: Symbol::from("now"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        ),
        // `self.<col> + 1` / `- 1`. Rails issues `SET col = col + 1` so
        // the increment is atomic in the database; this reads, adds and
        // writes, which is the same answer without a concurrent writer
        // and a lost update with one. Recorded in
        // docs/pipeline/runtime.md rather than left implicit.
        OpKind::Increment | OpKind::Decrement => Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(span, ExprNode::SelfRef)),
                        method: column.clone(),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                )),
                method: Symbol::from(if matches!(op.kind, OpKind::Increment) { "+" } else { "-" }),
                args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Int { value: 1 } })],
                block: None,
                parenthesized: false,
            },
        ),
    };
    let assign = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::SelfRef)),
            method: Symbol::from(format!("{}=", column.as_str())),
            args: vec![value],
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

#[derive(Clone, Copy, PartialEq)]
enum OpKind {
    Touch,
    Increment,
    Decrement,
}

struct ColumnOp {
    kind: OpKind,
    column: Symbol,
}

/// The column op named by a bare call on this model instance.
///
/// `increment!`/`decrement!` are claimed ONLY with `touch: true`, which
/// is what the corpus writes (campfire's `Membership::Connectable`) and
/// what this rewrite actually reproduces: the trailing `touch` is how
/// the new value reaches the row, and without it Rails would persist
/// the counter WITHOUT stamping `updated_at`. Claiming the bare form
/// would mean either a second persistence path or a silently wrong
/// timestamp; it keeps its NoMethodError instead.
fn column_op(e: &Expr) -> Option<ColumnOp> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else { return None };
    match recv {
        None => {}
        Some(r) if matches!(&*r.node, ExprNode::SelfRef) => {}
        Some(_) => return None,
    }
    let kind = match method.as_str() {
        "touch" => OpKind::Touch,
        "increment!" => OpKind::Increment,
        "decrement!" => OpKind::Decrement,
        _ => return None,
    };
    let ExprNode::Lit { value: Literal::Sym { value } } = &*args.first()?.node else {
        return None;
    };
    let column = value.clone();
    match kind {
        OpKind::Touch if args.len() == 1 => {}
        OpKind::Increment | OpKind::Decrement if args.len() == 2 => {
            let ExprNode::Hash { entries, .. } = &*args[1].node else { return None };
            let [(k, v)] = &entries[..] else { return None };
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                return None;
            };
            if key.as_str() != "touch" || !matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } }) {
                return None;
            }
        }
        _ => return None,
    }
    Some(ColumnOp { kind, column })
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

    /// `increment!(:col, touch: true)` is claimed; the bare form is
    /// not — see `column_op` for why a silently-unstamped counter is
    /// worse than a NoMethodError.
    #[test]
    fn rewrites_increment_with_touch_and_declines_without() {
        let call = |with_touch: bool| {
            let mut args = vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Sym { value: Symbol::from("connections") } },
            )];
            if with_touch {
                args.push(Expr::new(
                    Span::synthetic(),
                    ExprNode::Hash {
                        entries: vec![(
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit {
                                    value: Literal::Sym { value: Symbol::from("touch") },
                                },
                            ),
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit { value: Literal::Bool { value: true } },
                            ),
                        )],
                        kwargs: true,
                    },
                ));
            }
            Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("increment!"),
                    args,
                    block: None,
                    parenthesized: false,
                },
            )
        };

        let mut with_touch = call(true);
        rewrite_stmt(&mut with_touch);
        let ExprNode::Seq { exprs } = &*with_touch.node else {
            panic!("expected a Seq, got {:?}", with_touch.node)
        };
        assert_eq!(exprs.len(), 2);

        let mut bare = call(false);
        rewrite_stmt(&mut bare);
        assert!(matches!(&*bare.node, ExprNode::Send { .. }), "bare increment! is not claimed");
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
