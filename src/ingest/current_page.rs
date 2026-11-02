//! `current_page?(path)` is rewritten to the runtime's
//! `ActionView::ViewHelpers.current_page_match?(request.request_method,
//! request.path, request.fullpath, request.protocol +
//! request.host_with_port, path)`, which does
//! what Rails 8.1.4's helper does for a path or a URL: a GET or HEAD
//! request, both sides percent-decoded, the request's fullpath when the
//! path has a query, trailing `/` dropped, compared as bytes; a decoded
//! `scheme://…` (a literal, a computed `*_url`, or a percent-encoded one)
//! against the request's protocol and host in front of that path. The
//! runtime takes the request's parts as Strings: the shared runtime is
//! emitted on every target, and most have no request type for it to name.
//!
//! Left as written, and refused by `analyze::diagnostics`: the `url_for`
//! options form (`current_page?(controller: …)`), `check_parameters:` and
//! `method:`. A splat or forwarded arguments are left too, as is a call in
//! an `app/helpers` module (only views are rewritten). An app's own
//! `current_page?` is neither rewritten nor refused.

use crate::App;
use crate::expr::{Expr, ExprNode};
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

/// The `current_page_match?` calls this pass wrote, by span, each with
/// whether its view is one no request reaches (`App::unreachable_views`):
/// `project::report_current_page_calls` refuses them on the targets whose
/// views have no request. Reachability goes by the view's name, as a
/// view body's own span can be synthetic (a Jbuilder template's leading
/// `return if …` guard).
pub(crate) fn rewritten_calls(app: &App) -> Vec<(crate::span::Span, bool)> {
    fn visit(e: &Expr, unreachable: bool, out: &mut Vec<(crate::span::Span, bool)>) {
        if let ExprNode::Send { recv: Some(r), method, .. } = &*e.node
            && method.as_str() == "current_page_match?"
            && matches!(&*r.node, ExprNode::Const { path } if path.len() == 2 && path[0].as_str() == "ActionView" && path[1].as_str() == "ViewHelpers")
        {
            out.push((e.span, unreachable));
        }
        e.node.for_each_child(&mut |c| visit(c, unreachable, out));
    }
    let mut out = Vec::new();
    for view in &app.views {
        visit(&view.body, app.unreachable_views.contains(&view.name), &mut out);
    }
    out
}

fn rewrite(e: &mut Expr) {
    e.node.for_each_child_mut(&mut |child| rewrite(child));
    if let Some(built) = as_path_check(e) {
        *e = built;
    }
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
    if opaque || matches!(&*path.node, ExprNode::Hash { .. }) {
        return None;
    }
    let span = e.span;
    let send = |recv: Option<Expr>, method: &str, args: Vec<Expr>| {
        Expr::new(span, ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: false })
    };
    let request = |method: &str| send(Some(send(None, "request", Vec::new())), method, Vec::new());
    let origin = send(Some(request("protocol")), "+", vec![request("host_with_port")]);
    let helpers = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] });
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(helpers),
            method: Symbol::from("current_page_match?"),
            args: vec![request("request_method"), request("path"), request("fullpath"), origin, path.clone()],
            block: None,
            parenthesized: true,
        },
    ))
}
