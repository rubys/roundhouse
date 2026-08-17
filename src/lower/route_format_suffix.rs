//! `x_path(…, format: :json)` → `x_path(…) + ".json"`.
//!
//! Rails' `(.:format)` group means every route helper accepts a
//! `format:` option that appends an extension to the URL. It is not a
//! path SEGMENT and not a query key — it is a suffix — and that is why
//! it does not belong in the helper's signature.
//!
//! **Growing a parameter for it was the obvious move and the wrong
//! one.** Rust and Go have no default arguments, so a helper that gains
//! an optional `format` gains it for EVERY caller: real-blog's
//! `article_path(article.id)` stopped compiling in both targets the
//! moment one jbuilder line wrote `article_url(article, format: :json)`.
//! Widening a shared signature to serve a handful of call sites charges
//! the cost to all of them. Rewriting the call site charges it to the
//! caller that asked, and needs no signature at all.
//!
//! The same argument the `authenticate_by` lowering makes: Rails
//! partitions the option hash at RUNTIME, and that partition is a
//! compile-time fact here.
//!
//! Scope: bare `*_path` / `*_url` calls whose name matches a route in
//! the app's own table. `format:` on anything else is somebody else's
//! keyword. Applies before `lower_routes_to_library_functions`, which
//! surveys the same call sites for query keys — `format` is on its
//! `NON_QUERY_OPTIONS` list either way, so the two do not interact.

use crate::app::App;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::span::Span;

pub fn apply_route_format_suffix_lowering(app: &mut App) {
    let helpers = route_helper_names(app);
    if helpers.is_empty() {
        return;
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &helpers));
    for view in &mut app.views {
        rewrite(&mut view.body, &helpers);
    }
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &helpers);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &helpers);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &helpers);
        }
    }
}

/// Every `<as_name>_path` / `<as_name>_url` the app's routes define.
fn route_helper_names(app: &App) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for route in super::routes::flatten_routes(app) {
        if !route.named {
            continue;
        }
        out.insert(format!("{}_path", route.as_name));
        out.insert(format!("{}_url", route.as_name));
    }
    out
}

fn rewrite(expr: &mut Expr, helpers: &std::collections::HashSet<String>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, helpers));
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &mut *expr.node else {
        return;
    };
    if !helpers.contains(method.as_str()) {
        return;
    }
    let Some(last) = args.last_mut() else { return };
    let ExprNode::Hash { entries, kwargs: true } = &mut *last.node else {
        return;
    };
    let Some(idx) = entries.iter().position(|(k, _)| {
        matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
            if value.as_str() == "format")
    }) else {
        return;
    };
    let (_, format_value) = entries.remove(idx);
    // A hash that held ONLY `format:` is gone entirely; one that also
    // carried query keys (`account_users_path(page: page, format:
    // :turbo_stream)`) keeps them, and the helper keeps the query
    // parameter it was built with.
    if entries.is_empty() {
        args.pop();
    }
    let span = expr.span;
    // The rebuilt call keeps the ORIGINAL node's type. `Expr::new` seeds
    // `ty: None`, and dropping a type an earlier pass established makes
    // the call an `unresolved_type` in the ledger — a diagnostic about
    // this rewrite rather than about the app.
    let mut call = Expr::new(span, std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] }));
    call.ty = expr.ty.clone().or(Some(crate::ty::Ty::Str));
    // A literal folds into the text; anything else interpolates, which
    // is where `to_s` on a Symbol happens.
    let suffix = match &*format_value.node {
        ExprNode::Lit { value: Literal::Sym { value } } => {
            crate::lower::typing::lit_str(format!(".{}", value.as_str()))
        }
        ExprNode::Lit { value: Literal::Str { value } } => {
            crate::lower::typing::lit_str(format!(".{value}"))
        }
        _ => crate::lower::typing::with_ty(
            Expr::new(
                span,
                ExprNode::StringInterp {
                    parts: vec![
                        InterpPart::Text { value: ".".to_string() },
                        InterpPart::Expr { expr: format_value.clone() },
                    ],
                },
            ),
            crate::ty::Ty::Str,
        ),
    };
    *expr.node = ExprNode::Send {
        recv: Some(call),
        method: Symbol::from("+"),
        args: vec![suffix],
        block: None,
        parenthesized: false,
    };
    expr.ty = Some(crate::ty::Ty::Str);
}
