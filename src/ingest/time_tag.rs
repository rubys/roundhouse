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
//! `t.iso8601` before that value wins), have a computed key or a computed
//! `format:` value (Rails evaluates it), whose text is a literal `nil` or
//! `false` (Rails falls back to its default text), whose block takes
//! parameters, or that has a block, no text and a `format:` (Rails still
//! localizes with it) is left as written and refused. Analysis, the time
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
    if let Some(Ok(built)) = lowering(e) {
        calls.push(e.span);
        *e = built;
    }
}

const NOT_MODELED: &str = "its arguments are not a time, an optional text and a literal options hash";
const NO_TEXT: &str = "with no text argument and no block, Rails' text is `I18n.l(t, format: …)`, which is not modeled; pass the text (`time_tag t, \"…\"`) or a block";

/// A `time_tag` call's `tag.time` form, or why it is left as written (the
/// reason `analyze::diagnostics` refuses it with); `None` for any other
/// expression.
pub(crate) fn lowering(e: &Expr) -> Option<Result<Expr, &'static str>> {
    let ExprNode::Send { recv: None, method, args, block, .. } = &*e.node else { return None };
    if method.as_str() != "time_tag" {
        return None;
    }
    Some(as_tag_time(e.span, args, block.as_ref()))
}

fn as_tag_time(span: crate::span::Span, args: &[Expr], block: Option<&Expr>) -> Result<Expr, &'static str> {
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
        return Err(NOT_MODELED);
    }
    let Some((time, rest)) = args.split_first() else { return Err(NO_TEXT) };
    let (positional, opts): (&[Expr], Vec<(Expr, Expr)>) = match rest.split_last() {
        Some((last, before)) => match &*last.node {
            ExprNode::Hash { entries, .. } => (before, entries.clone()),
            _ => (rest, Vec::new()),
        },
        None => (rest, Vec::new()),
    };
    // One text argument at most, and it is never a hash.
    if positional.len() > 1 || positional.iter().any(|p| matches!(&*p.node, ExprNode::Hash { .. })) {
        return Err(NOT_MODELED);
    }
    let text = positional.first().cloned();
    if text.is_none() && block.is_none() {
        return Err(NO_TEXT);
    }
    // Rails' text is `args.first || I18n.l(…)`: a `nil` or `false` one
    // falls back to that, which is not modeled.
    if text.as_ref().is_some_and(|t| matches!(&*t.node, ExprNode::Lit { value: Literal::Nil | Literal::Bool { value: false } })) {
        return Err("a `nil` or `false` text falls back to Rails' `I18n.l(t, format: …)`, which is not modeled; pass the text or a block");
    }
    // `content_tag` captures the block with no arguments; a block that
    // names parameters has nothing the view builder binds them to.
    if let Some(b) = block
        && let ExprNode::Lambda { params, rest_param, extra_params, block_param, .. } = &*b.node
        && (!params.is_empty() || rest_param.is_some() || !extra_params.is_empty() || block_param.is_some())
    {
        return Err("a block with parameters is not modeled; `content_tag` passes it none");
    }
    // Literal keys only: a computed one could be `:format` or `:datetime`.
    if opts.iter().any(|(k, _)| !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { .. } | Literal::Str { .. } })) {
        return Err(NOT_MODELED);
    }
    // With no text, Rails still makes the default text, `I18n.l(t, format:
    // …)`, before it captures the block; with a given `format:` that can
    // raise (a format the locale lacks), which is not modeled.
    if text.is_none() && opts.iter().any(|(k, _)| matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "format")) {
        return Err("with a block and no text, Rails still localizes the time with the given `format:` (`I18n.l`), which raises for a format the locale lacks; that is not modeled");
    }
    // Rails deletes and merges Symbol keys; a String "format" or "datetime"
    // stays an attribute beside them (`datetime="…Z" datetime="2026"`),
    // which the builder's attributes don't keep apart: left as written.
    let is_sym = |k: &Expr, name: &str| matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == name);
    if opts.iter().any(|(k, _)| matches!(&*k.node, ExprNode::Lit { value: Literal::Str { value } } if value == "datetime" || value == "format")) {
        return Err(NOT_MODELED);
    }
    // A given `datetime:` wins over `t.iso8601`, but Rails still evaluates
    // that first (it raises for a nil time): left as written and refused.
    if opts.iter().any(|(k, _)| is_sym(k, "datetime")) {
        return Err("a given `datetime:` wins over the time's `iso8601`, which Rails still evaluates first; that order is not modeled");
    }
    // `format:` is dropped, but Rails evaluates its value when it builds
    // the options: only a literal one can go without that.
    if opts.iter().any(|(k, v)| is_sym(k, "format") && !matches!(&*v.node, ExprNode::Lit { .. })) {
        return Err("a computed `format:` is evaluated by Rails though the option is dropped; pass a literal");
    }
    let opts: Vec<(Expr, Expr)> = opts.into_iter().filter(|(k, _)| !is_sym(k, "format")).collect();
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
    Ok(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(tag),
            method: Symbol::from("time"),
            args: call_args,
            block: block.cloned(),
            parenthesized: true,
        },
    ))
}
