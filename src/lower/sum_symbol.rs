//! Rails collection `sum(:column)` grounding: `X.sum(:hotness_mod)` →
//! `X.to_a.sum { |__r| __r.hotness_mod }` (lobsters'
//! Story#calculated_hotness over its staged/loaded tags). The symbol
//! form is Rails collection API — on a runtime Relation the SQL path
//! serves it, but a collection writer stages a plain Array into the
//! association cache, and core `Array#sum(:sym)` reads the symbol as
//! an INIT value (`:sym + record` → NoMethodError). The block form is
//! uniform: `to_a` is identity on Array and materializes a Relation,
//! and the per-record read needs no runtime reflection. A String arg
//! (`sum("comments.score + 1")`) stays verbatim — that's the
//! SQL-expression form only the Relation path can serve.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn apply_sum_symbol_lowering(app: &mut App) {
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    rewrite(&mut method.body);
                }
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => rewrite(expr),
                _ => {}
            }
        }
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    let sym = match &*expr.node {
        ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if method.as_str() == "sum" && args.len() == 1 =>
        {
            match &*args[0].node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            }
        }
        _ => None,
    };
    let Some(col) = sym else { return };
    let ExprNode::Send { recv, .. } = &mut *expr.node else { return };
    let receiver = recv.take().unwrap();
    let to_a = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(receiver),
            method: Symbol::from("to_a"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let record = Symbol::from("__r");
    let read = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Var { id: crate::ident::VarId(0), name: record.clone() },
            )),
            method: col,
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let lambda = Expr::new(
        span,
        ExprNode::Lambda {
            params: vec![record],
            block_param: None,
            body: read,
            block_style: crate::expr::BlockStyle::Brace,
        },
    );
    *expr.node = ExprNode::Send {
        recv: Some(to_a),
        method: Symbol::from("sum"),
        args: vec![],
        block: Some(lambda),
        parenthesized: false,
    };
    expr.ty = None;
}
