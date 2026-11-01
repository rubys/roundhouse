//! `current_page?(path)` is rewritten to the runtime's
//! `ActionView::ViewHelpers.current_page_path?(request, path)`, which does
//! what Rails 8.1.4's helper does for a path: a GET or HEAD request, both
//! sides percent-decoded, the request's fullpath when the path has a query,
//! trailing `/` dropped, compared as bytes.
//!
//! Left as written, and refused by `analyze::diagnostics`: the `url_for`
//! options form (`current_page?(controller: …)`), `check_parameters:` and
//! `method:`, and a literal absolute URL, which Rails compares against the
//! request's protocol and host. A computed path is compared as given.

use crate::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

pub fn lower_current_page(app: &mut App) {
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
    for lc in &mut app.library_classes {
        for method in &mut lc.methods {
            rewrite(&mut method.body);
        }
    }
}

/// The `current_page_path?` calls this pass wrote, by span, in every view
/// and library class: `project::report_current_page_calls` refuses them
/// on the targets whose views have no request.
pub(crate) fn rewritten_calls(app: &App) -> Vec<crate::span::Span> {
    fn visit(e: &Expr, out: &mut Vec<crate::span::Span>) {
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node
            && method.as_str() == "current_page_path?"
            && matches!(&*r.node, ExprNode::Const { path } if path.len() == 2 && path[0].as_str() == "ActionView" && path[1].as_str() == "ViewHelpers")
        {
            out.push(e.span);
        }
        e.node.for_each_child(&mut |c| visit(c, out));
    }
    let mut out = Vec::new();
    for view in &app.views {
        visit(&view.body, &mut out);
    }
    for lc in &app.library_classes {
        for method in &lc.methods {
            visit(&method.body, &mut out);
        }
    }
    out
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut |child| rewrite(child));
    if let Some(built) = as_path_check(e) {
        *e = built;
    }
}

/// A literal `scheme://…` URL.
pub(crate) fn is_absolute_url(e: &Expr) -> bool {
    let ExprNode::Lit { value: Literal::Str { value } } = &*e.node else { return false };
    let Some(at) = value.find("://") else { return false };
    at > 0 && value[..at].chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn as_path_check(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else { return None };
    if method.as_str() != "current_page?" {
        return None;
    }
    let [path] = args.as_slice() else { return None };
    if matches!(&*path.node, ExprNode::Hash { .. }) || is_absolute_url(path) {
        return None;
    }
    let span = e.span;
    let request = Expr::new(
        span,
        ExprNode::Send { recv: None, method: Symbol::from("request"), args: Vec::new(), block: None, parenthesized: false },
    );
    let helpers = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] });
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(helpers),
            method: Symbol::from("current_page_path?"),
            args: vec![request, path.clone()],
            block: None,
            parenthesized: true,
        },
    ))
}
