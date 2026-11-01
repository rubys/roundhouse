//! `time_tag(t, "text", **opts)` and `time_tag(t, **opts) do … end` are
//! rewritten to the builder call Rails' helper makes (actionview 8.1.4,
//! `DateHelper#time_tag`):
//!
//! ```ruby
//! content_tag("time", content, options.reverse_merge(datetime: date_or_time.iso8601), &block)
//! ```
//!
//! that is `tag.time("text", datetime: t.iso8601, **opts)`, with `datetime`
//! first and `format:` dropped (Rails deletes it; it only picks the default
//! text). A call whose options give `datetime:` (Rails still evaluates
//! `t.iso8601` before that value wins) or have a computed key is left as
//! written and refused. Analysis, the time
//! lowerings and the tag builder then see what an app writing the builder
//! by hand would have written.
//!
//! The default text, `I18n.l(t, format: …)` when there is neither a text
//! argument nor a block, is not modeled: such a call is left as written and
//! `analyze::diagnostics` refuses it, as it does a call whose options are not
//! a literal hash. A rewritten call's span is kept (`App::time_tag_calls`)
//! for the targets that refuse it.

use crate::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// The app's own `time_tag` (an `app/helpers` method, or a controller's
/// `helper_method`), which Rails calls ahead of ActionView's.
pub(crate) fn app_defines_time_tag(app: &App) -> bool {
    let name = Symbol::from("time_tag");
    app.helper_method_index.contains_key(&name) || app.view_visible_controller_methods.contains(&name)
}

pub fn lower_time_tags(app: &mut App) {
    if app_defines_time_tag(app) {
        return;
    }
    let mut calls = Vec::new();
    for view in &mut app.views {
        rewrite(&mut view.body, &mut calls);
    }
    for lc in &mut app.library_classes {
        for method in &mut lc.methods {
            rewrite(&mut method.body, &mut calls);
        }
    }
    app.time_tag_calls = calls;
}

fn rewrite(e: &mut Expr, calls: &mut Vec<crate::span::Span>) {
    e.node.for_each_child_mut(&mut |child| rewrite(child, calls));
    if let Some(built) = as_tag_time(e) {
        calls.push(e.span);
        *e = built;
    }
}

fn as_tag_time(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block, .. } = &*e.node else { return None };
    if method.as_str() != "time_tag" {
        return None;
    }
    // A splat or forwarded keywords: not a time, a text and literal options.
    let opaque = |a: &Expr| {
        matches!(
            &*a.node,
            ExprNode::Splat { .. }
                | ExprNode::KeywordSplat { .. }
                | ExprNode::ForwardArgs
                | ExprNode::ForwardKeywords
                | ExprNode::ForwardKeywordsWithPairs { .. }
        )
    };
    if args.iter().any(opaque)
        || args.last().is_some_and(|a| matches!(&*a.node, ExprNode::Hash { entries, .. } if entries.iter().any(|(k, v)| opaque(k) || opaque(v))))
    {
        return None;
    }
    let (time, rest) = args.split_first()?;
    let (positional, opts): (&[Expr], Vec<(Expr, Expr)>) = match rest.split_last() {
        Some((last, before)) => match &*last.node {
            ExprNode::Hash { entries, .. } => (before, entries.clone()),
            _ => (rest, Vec::new()),
        },
        None => (rest, Vec::new()),
    };
    // One text argument at most, and it is never a hash.
    if positional.len() > 1 || positional.iter().any(|p| matches!(&*p.node, ExprNode::Hash { .. })) {
        return None;
    }
    let text = positional.first().cloned();
    if text.is_none() && block.is_none() {
        return None;
    }
    // Literal keys only: a computed one could be `:format` or `:datetime`.
    if opts.iter().any(|(k, _)| !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } })) {
        return None;
    }
    // Rails deletes and merges Symbol keys; a String "format" or "datetime"
    // stays an attribute beside them (`datetime="…Z" datetime="2026"`),
    // which the builder's attributes don't keep apart: left as written.
    let is_sym = |k: &Expr, name: &str| matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name);
    if opts.iter().any(|(k, _)| matches!(&*k.node, ExprNode::Lit { value: Literal::Str { value } } if value == "datetime" || value == "format")) {
        return None;
    }
    // A given `datetime:` wins over `t.iso8601`, but Rails still evaluates
    // that first (it raises for a nil time): left as written and refused.
    if opts.iter().any(|(k, _)| is_sym(k, "datetime")) {
        return None;
    }
    let opts: Vec<(Expr, Expr)> = opts.into_iter().filter(|(k, _)| !is_sym(k, "format")).collect();
    let span = e.span;
    // `options.reverse_merge(datetime: …)` is `{ datetime: … }.merge(options)`:
    // `datetime` first.
    let mut entries = Vec::new();
    {
        let iso = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(time.clone()),
                method: Symbol::from("iso8601"),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        );
        entries.push((Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from("datetime") } }), iso));
    }
    entries.extend(opts);
    let tag = Expr::new(
        span,
        ExprNode::Send { recv: None, method: Symbol::from("tag"), args: Vec::new(), block: None, parenthesized: false },
    );
    let mut call_args: Vec<Expr> = text.into_iter().collect();
    call_args.push(Expr::new(span, ExprNode::Hash { entries, kwargs: true }));
    Some(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(tag),
            method: Symbol::from("time"),
            args: call_args,
            block: block.clone(),
            parenthesized: true,
        },
    ))
}
