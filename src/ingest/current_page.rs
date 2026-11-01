//! `current_page?(path)` is rewritten to the runtime's
//! `ActionView::ViewHelpers.current_page_path?(request, path)`, which does
//! what Rails 8.1.4's helper does for a path: a GET or HEAD request, both
//! sides percent-decoded, the request's fullpath when the path has a query,
//! trailing `/` dropped, compared as bytes.
//!
//! Left as written, and refused by `analyze::diagnostics`: the `url_for`
//! options form (`current_page?(controller: …)`), `check_parameters:` and
//! `method:`, and a literal absolute URL, which Rails compares against the
//! request's protocol and host. A splat or forwarded arguments are left
//! too, as is a call in an `app/helpers` module (only views are rewritten).
//! A computed path is compared as given. An app's own `current_page?` is
//! neither rewritten nor refused.

use crate::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// The app's own `current_page?` (an `app/helpers` method, or a
/// controller's `helper_method`), which Rails calls ahead of ActionView's.
pub(crate) fn app_defines_current_page(app: &App) -> bool {
    let name = Symbol::from("current_page?");
    app.helper_method_index.contains_key(&name) || app.view_visible_controller_methods.contains(&name)
}

/// The bodies ActionView's `current_page?` can be called from: the views,
/// and the `app/helpers` modules (Rails runs a helper method in the view's
/// context). Other library classes are left alone: their own
/// `current_page?` is theirs.
pub(crate) fn for_each_view_context_body(app: &App, f: &mut impl FnMut(&Expr)) {
    for view in &app.views {
        f(&view.body);
    }
    for lc in app.library_classes.iter().filter(|lc| app.helper_modules.contains(&lc.name)) {
        for method in &lc.methods {
            f(&method.body);
        }
    }
}

pub fn lower_current_page(app: &mut App) {
    if app_defines_current_page(app) {
        return;
    }
    // Views only: in an `app/helpers` module the runtime's
    // `ActionView::ViewHelpers` is not a constant analysis resolves, so a
    // helper's `current_page?` is left as written and refused.
    for view in &mut app.views {
        rewrite(&mut view.body);
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
    for_each_view_context_body(app, &mut |body| visit(body, &mut out));
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
    // A splat or forwarded arguments may be anything, the options form included.
    let opaque = matches!(
        &*path.node,
        ExprNode::Splat { .. }
            | ExprNode::KeywordSplat { .. }
            | ExprNode::ForwardArgs
            | ExprNode::ForwardKeywords
            | ExprNode::ForwardKeywordsWithPairs { .. }
    );
    if opaque || matches!(&*path.node, ExprNode::Hash { .. }) || is_absolute_url(path) {
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
