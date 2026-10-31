//! ActionView's render events for the lowered views: each view body is
//! bracketed by `ActiveSupport::Notifications.render_start` /
//! `render_finish("render_template.action_view", "app/views/…", t)`
//! (`render_partial` for a partial, `render_layout` under `layouts/`), and
//! a `render collection` loop by `collection_start` / `collection_finish`,
//! which reports the collection once and its items not at all, as Rails'
//! collection renderer does. The runtime half is
//! `runtime/spinel/active_support_instrumentation.rb`; with no subscriber
//! each bracket is one flag read.
//!
//! Ruby-family only, like `view_buffer_passing`, whose split it runs
//! before so the brackets land in the `_into` variant and not twice.

use crate::dialect::{LibraryClass, View};
use crate::expr::{BlockStyle, Expr, ExprNode, IrHint, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

const START: &str = "__render_started";
const COLLECTION: &str = "__collection_started";

fn notifications(method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("ActiveSupport"), Symbol::from("Notifications")] },
            )),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

fn str_lit(s: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
}

fn var(name: &str) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name: Symbol::from(name) })
}

fn assign(name: &str, value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign { target: LValue::Var { id: VarId(0), name: Symbol::from(name) }, value },
    )
}

/// `app/views/stories/_listdetail.html.erb`: Rails' identifier is the absolute path, and its last two segments are all a subscriber is shown.
fn identifier(view: &View, app: &crate::App) -> String {
    if let Some(path) = app.source_path(view.body.span.file) {
        if let Some(at) = path.rfind("app/views/") {
            return path[at..].to_string();
        }
    }
    format!("app/views/{}.{}.erb", view.name.as_str(), view.format.as_str())
}

fn event(view: &View) -> &'static str {
    let (dir, base) = crate::lower::view_to_library::split_view_name(view.name.as_str());
    if dir == "layouts" || dir.starts_with("layouts/") {
        "render_layout.action_view"
    } else if base.starts_with('_') {
        "render_partial.action_view"
    } else {
        "render_template.action_view"
    }
}

fn accumulator_shaped(exprs: &[Expr]) -> bool {
    exprs.len() >= 2
        && exprs.first().is_some_and(|e| e.hint() == &Some(IrHint::StringBuilderInit))
        && exprs.last().is_some_and(|e| e.hint() == &Some(IrHint::StringBuilderResult))
}

pub fn apply(views: &[&View], lcs: &mut [LibraryClass], app: &crate::App) {
    let mut identifiers: crate::det::HashMap<(String, String), String> = Default::default();
    for (view, lc) in views.iter().zip(lcs.iter()) {
        let (_, base) = crate::lower::view_to_library::split_view_name(view.name.as_str());
        let method = crate::lower::view::view_method_name_for(base.trim_start_matches('_'), view.format.as_str());
        identifiers.insert((lc.name.0.as_str().to_string(), method.as_str().to_string()), identifier(view, app));
    }
    for (view, lc) in views.iter().zip(lcs.iter_mut()) {
        let (_, base) = crate::lower::view_to_library::split_view_name(view.name.as_str());
        let method = crate::lower::view::view_method_name_for(base.trim_start_matches('_'), view.format.as_str());
        let id = identifier(view, app);
        for m in lc.methods.iter_mut() {
            let mut n = 0;
            wrap_collections(&mut m.body, &identifiers, &mut n);
            if m.name != method {
                continue;
            }
            let ExprNode::Seq { exprs } = &mut *m.body.node else { continue };
            if !accumulator_shaped(exprs) {
                continue;
            }
            let last = exprs.len() - 1;
            exprs.insert(last, notifications("render_finish", vec![str_lit(event(view)), str_lit(&id), var(START)]));
            exprs.insert(1, assign(START, notifications("render_start", Vec::new())));
        }
    }
}

/// The loop `partial.rs` writes for `render collection` (a brace block whose one statement appends the partial for its element) — an ERB `each do` written by the app is a `Do` block and is left alone.
fn collection_partial<'a>(e: &'a Expr) -> Option<(&'a [Symbol], &'a str)> {
    let ExprNode::Send { method, block: Some(block), .. } = &*e.node else { return None };
    if method.as_str() != "each" {
        return None;
    }
    let ExprNode::Lambda { params, body, block_style: BlockStyle::Brace, .. } = &*block.node else { return None };
    let [param] = params.as_slice() else { return None };
    let ExprNode::Send { method: append, args, .. } = &*body.node else { return None };
    if append.as_str() != "<<" || args.len() != 1 {
        return None;
    }
    let ExprNode::Send { recv: Some(recv), method: partial, args: call_args, .. } = &*args[0].node else { return None };
    let ExprNode::Const { path } = &*recv.node else { return None };
    if path.first().map(|s| s.as_str()) != Some("Views") {
        return None;
    }
    let first_is_element = call_args.first().is_some_and(|a| matches!(&*a.node, ExprNode::Var { name, .. } if name == param));
    first_is_element.then_some((path.as_slice(), partial.as_str()))
}

fn wrap_collections(e: &mut Expr, identifiers: &crate::det::HashMap<(String, String), String>, n: &mut usize) {
    e.node.for_each_child_mut(&mut |c| wrap_collections(c, identifiers, n));
    let Some((path, partial)) = collection_partial(e) else { return };
    let key = (path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"), partial.to_string());
    let Some(id) = identifiers.get(&key).cloned() else { return };
    // Not one local for every loop: a collection nested in another's partial call would overwrite the outer start.
    let local = format!("{COLLECTION}{n}");
    *n += 1;
    let span = e.span;
    let looped = e.clone();
    *e = Expr::new(
        span,
        ExprNode::Seq {
            exprs: vec![
                assign(&local, notifications("collection_start", Vec::new())),
                looped,
                notifications("collection_finish", vec![str_lit(&id), var(&local)]),
            ],
        },
    );
}
