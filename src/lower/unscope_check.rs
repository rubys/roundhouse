//! Refuse an `unscope` the runtime cannot honour.
//!
//! The shared `Relation#unscope` implements only `unscope(where: <column>)`
//! and `unscope(where: [<column>, ...])`; `unscope(:where)` is rewritten to
//! `Relation#unscope_where` first. Anything else -- `unscope(:order)`,
//! `unscope(where: nil)`, a non-literal target -- is
//! accepted by analysis (the method resolves) but raises at run time, or
//! would silently remove nothing. A removed error is a claim the program
//! runs, so those shapes are an Error here rather than a late surprise.

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// `unscope(:where)` drops every WHERE condition: `Relation#unscope_where`.
pub fn rewrite_unscope_where(app: &mut App) {
    fn rewrite(e: &mut Expr) {
        e.node.for_each_child_mut(&mut rewrite);
        if let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &mut *e.node
            && method.as_str() == "unscope"
            && matches!(args.as_slice(), [arg] if matches!(&*arg.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "where"))
        {
            *method = Symbol::from("unscope_where");
            args.clear();
        }
    }
    super::for_each_hook_body(app, &mut rewrite);
}

pub fn apply_unscope_check(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let seen: std::cell::RefCell<Vec<crate::span::Span>> = std::cell::RefCell::new(Vec::new());
    super::for_each_hook_body_ref(app, &mut |body| {
        let _ = crate::lower::controller_to_library::util::map_expr(body, &|e| {
            if let ExprNode::Send { method, args, .. } = &*e.node {
                if method.as_str() == "unscope" && !is_supported(args) {
                    seen.borrow_mut().push(e.span);
                }
            }
            None
        });
    });
    for span in seen.borrow().iter() {
        let kind = DiagnosticKind::Unsupported {
            target: None,
            construct: Symbol::from("unscope"),
            detail: "only unscope(where: :column) is supported".into(),
        };
        out.push(Diagnostic {
            span: *span,
            severity: Diagnostic::default_severity(&kind),
            kind,
            message: "`unscope` is supported only as `unscope(where: :column)` or \
                      `unscope(where: [:a, :b])` with literal column names; this form \
                      would raise or remove nothing at run time"
                .into(),
        });
    }
    out
}

fn is_column_literal(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Sym { .. } } | ExprNode::Lit { value: Literal::Str { .. } }
    )
}

fn is_supported(args: &[Expr]) -> bool {
    let [arg] = args else { return false };
    let ExprNode::Hash { entries, .. } = &*arg.node else { return false };
    let [(k, v)] = entries.as_slice() else { return false };
    let is_where = matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "where");
    if !is_where {
        return false;
    }
    match &*v.node {
        ExprNode::Array { elements, .. } => elements.iter().all(is_column_literal),
        _ => is_column_literal(v),
    }
}
