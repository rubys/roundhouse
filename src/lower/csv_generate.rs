// Not passed through: spinel's `csv` package takes no `headers:` / `write_headers:`, so the header row is written as the block's first row.
use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

pub fn apply_csv_generate_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    rewrite_node(expr);
}

pub(crate) fn rewrite_node(expr: &mut Expr) {
    let ExprNode::Send {
        recv: Some(r),
        method,
        args,
        block: Some(block),
        ..
    } = &mut *expr.node
    else {
        return;
    };
    if method.as_str() != "generate"
        || !matches!(&*r.node, ExprNode::Const { path } if path.last().is_some_and(|s| s.as_str() == "CSV"))
    {
        return;
    }
    // A block with optional or keyword parameters is not the `|csv|` shape;
    // decline before any option is taken off the call.
    if matches!(&*block.node, ExprNode::Lambda { extra_params, .. } if !extra_params.is_empty()) {
        return;
    }
    let Some(ExprNode::Hash { entries, .. }) = args.last_mut().map(|a| &mut *a.node) else {
        return;
    };
    let take = |entries: &mut Vec<(Expr, Expr)>, name: &str| -> Option<Expr> {
        let i = entries
            .iter()
            .position(|(k, _)| matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name))?;
        Some(entries.remove(i).1)
    };
    let headers = take(entries, "headers");
    let write = take(entries, "write_headers");
    if entries.is_empty() {
        args.pop();
    }
    let (Some(headers), Some(write)) = (headers, write) else {
        return;
    };
    if matches!(
        &*write.node,
        ExprNode::Lit {
            value: Literal::Bool { value: false }
        } | ExprNode::Lit {
            value: Literal::Nil
        }
    ) {
        return;
    }
    let ExprNode::Lambda { params, body, .. } = &mut *block.node else {
        return;
    };
    let Some(csv) = params.first().cloned() else {
        return;
    };
    let span = body.span;
    let csv_ty = Ty::Class {
        id: ClassId(Symbol::from("CSV")),
        args: vec![].into(),
    };
    let mut target = Expr::new(
        span,
        ExprNode::Var {
            id: crate::ident::VarId(0),
            name: csv,
        },
    );
    target.ty = Some(csv_ty.clone());
    let mut row = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(target),
            method: Symbol::from("<<"),
            args: vec![headers],
            block: None,
            parenthesized: false,
        },
    );
    row.ty = Some(csv_ty);
    if !matches!(
        &*write.node,
        ExprNode::Lit {
            value: Literal::Bool { value: true }
        }
    ) {
        row = Expr::new(
            span,
            ExprNode::If {
                cond: write,
                then_branch: row,
                else_branch: Expr::new(
                    span,
                    ExprNode::Lit {
                        value: Literal::Nil,
                    },
                ),
            },
        );
    }
    let rest = std::mem::replace(body, Expr::new(span, ExprNode::Seq { exprs: vec![] }));
    let mut exprs = vec![row];
    match *rest.node {
        ExprNode::Seq { exprs: inner } => exprs.extend(inner),
        other => exprs.push(Expr {
            node: Box::new(other),
            ..rest
        }),
    }
    *body.node = ExprNode::Seq { exprs };
}
