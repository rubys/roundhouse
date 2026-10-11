//! `t("key", …)` in a template and `I18n.t("key", …)` anywhere → the
//! String Rails' I18n answers from the app's locale (`crate::i18n`).
//!
//! The lookup happens at compile time: a literal key (or a view's lazy
//! `.key`), literal `scope:` / `default:`, and interpolation values that
//! are any expression. `count:` keeps the choice between plural forms
//! for run time. A template's `_html` key renders unescaped with its
//! values escaped, as `HtmlSafeTranslation` does. Anything else (a
//! dynamic key, `locale:`, a key the locale lacks, whose answer Rails
//! makes per environment by `raise_on_missing_translations`) is an error
//! in analyze, since no runtime carries the locale table; only what
//! `parse` and `Catalog::resolve` accept is typed, and only that folds.

use crate::app::App;
use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::i18n::{Catalog, Fallback, Piece, Translate, Translation};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// A translate call's lookup, and the expressions its `count:` and
/// interpolation values read.
pub(crate) struct Call {
    pub lookup: Translate,
    count: Option<Expr>,
    values: Vec<(String, Expr)>,
    /// `count` and the value names, in source order.
    order: Vec<String>,
}

/// `None` when this is not a translate call; `Err` when it is one whose
/// shape cannot be resolved at compile time.
pub(crate) fn parse(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    in_view: bool,
    view: Option<&str>,
) -> Option<Result<Call, String>> {
    if !matches!(method, "t" | "translate") {
        return None;
    }
    let on_i18n = matches!(recv.map(|r| &*r.node),
        Some(ExprNode::Const { path }) if path.len() == 1 && path[0].as_str() == "I18n");
    if !(on_i18n || (recv.is_none() && in_view)) {
        return None;
    }
    if block.is_some() {
        return Some(Err("a block on the call".into()));
    }
    Some(parse_args(args, view))
}

fn literal_text(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        _ => None,
    }
}

fn parse_args(args: &[Expr], view: Option<&str>) -> Result<Call, String> {
    let (key, opts) = match args {
        [key] => (key, None),
        [key, opts] => (key, Some(opts)),
        _ => return Err("an argument shape other than (key, **options)".into()),
    };
    let key = literal_text(key).ok_or("a key computed at run time")?;
    let mut call = Call {
        lookup: Translate { key, view: view.map(str::to_string), ..Default::default() },
        count: None,
        values: Vec::new(),
        order: Vec::new(),
    };
    let Some(opts) = opts else { return Ok(call) };
    let ExprNode::Hash { entries, .. } = &*opts.node else {
        return Err("options that are not a literal Hash".into());
    };
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
            return Err("an option key that is not a Symbol".into());
        };
        match name.as_str() {
            "scope" => {
                let parts: Option<Vec<String>> = match &*v.node {
                    ExprNode::Array { elements, .. } => elements.iter().map(literal_text).collect(),
                    _ => literal_text(v).map(|s| vec![s]),
                };
                call.lookup.scope = Some(parts.ok_or("a scope computed at run time")?.join("."));
            }
            "default" => {
                let one = |e: &Expr| match &*e.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(Fallback::Key(value.as_str().to_string())),
                    ExprNode::Lit { value: Literal::Str { value } } => Some(Fallback::Text(value.clone())),
                    _ => None,
                };
                let defaults: Option<Vec<Fallback>> = match &*v.node {
                    ExprNode::Array { elements, .. } => elements.iter().map(one).collect(),
                    _ => one(v).map(|d| vec![d]),
                };
                call.lookup.defaults = defaults.ok_or("a default computed at run time")?;
            }
            "count" => {
                call.lookup.counted = true;
                call.count = Some(v.clone());
                call.order.push("count".into());
            }
            "locale" | "raise" | "throw" | "separator" | "exception_handler" | "fallback" => {
                return Err(format!("the `{name}:` option"));
            }
            _ => {
                call.lookup.values.push(name.as_str().to_string());
                call.values.push((name.as_str().to_string(), v.clone()));
                call.order.push(name.as_str().to_string());
            }
        }
    }
    Ok(call)
}

/// Fold every resolvable translate call in hook bodies, test bodies and
/// templates.
pub fn apply_i18n_translate_lowering(app: &mut App) {
    let catalog = app.i18n.or_rails_default().clone();
    super::for_each_hook_body(app, &mut |body| rewrite(body, &catalog, false, None));
    super::for_each_test_body(app, &mut |body| rewrite(body, &catalog, false, None));
    for view in &mut app.views {
        let name = view.name.as_str().to_string();
        rewrite(&mut view.body, &catalog, true, Some(&name));
    }
}

fn rewrite(expr: &mut Expr, catalog: &Catalog, in_view: bool, view: Option<&str>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, catalog, in_view, view));
    let ExprNode::Send { recv, method, args, block, .. } = &*expr.node else { return };
    let Some(Ok(mut call)) = parse(recv.as_ref(), method.as_str(), args, block.as_ref(), in_view, view) else { return };
    let Ok(translation) = catalog.resolve(&call.lookup) else { return };
    let span = expr.span;
    let html = in_view && crate::i18n::is_html_safe_key(&call.lookup.key);
    let bound = bind_options(&mut call, &translation, span);
    let render = |template: &str| interpolate(span, template, &call, html);
    let mut folded = match (translation, &call.count) {
        (Translation::One(t), _) => render(&t),
        (Translation::Plural { zero, one, other }, Some(count)) => {
            let is = |n: i64, then: Expr, els: Expr| {
                let mut cond = Expr::new(span, ExprNode::Send {
                    recv: Some(count.clone()),
                    method: Symbol::from("=="),
                    args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Int { value: n } })],
                    block: None,
                    parenthesized: false,
                });
                cond.ty = Some(Ty::Bool);
                let mut e = Expr::new(span, ExprNode::If { cond, then_branch: then, else_branch: els });
                e.ty = Some(Ty::Str);
                e
            };
            let plural = is(1, render(&one), render(&other));
            match zero {
                Some(z) => is(0, render(&z), plural),
                None => plural,
            }
        }
        (Translation::Plural { .. }, None) => return,
    };
    folded.ty = Some(Ty::Str);
    if !bound.is_empty() {
        folded = typed(Expr::new(span, ExprNode::Seq { exprs: bound.into_iter().chain([folded]).collect() }));
    }
    if html {
        folded = typed(Expr::new(span, ExprNode::Send {
            recv: None,
            method: Symbol::from("raw"),
            args: vec![folded],
            block: None,
            parenthesized: true,
        }));
    }
    *expr = folded;
}

fn typed(mut e: Expr) -> Expr {
    e.ty = Some(Ty::Str);
    e
}

/// `ERB::Util.html_escape(value.to_s)`, as an HTML-safe key's values read.
fn escaped(span: Span, value: Expr) -> Expr {
    let to_s = typed(Expr::new(span, ExprNode::Send {
        recv: Some(value),
        method: Symbol::from("to_s"),
        args: vec![],
        block: None,
        parenthesized: false,
    }));
    typed(Expr::new(span, ExprNode::Send {
        recv: Some(Expr::new(span, ExprNode::Const {
            path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
        })),
        method: Symbol::from("html_escape"),
        args: vec![to_s],
        block: None,
        parenthesized: true,
    }))
}

/// Ruby evaluates each keyword value once, in order, whether or not the
/// translation reads it; folding can repeat, drop or reorder them, so
/// once any option is not a plain read, every option that is not a
/// literal is bound to a local first, in source order. The one exception
/// is a sole such option among literals, read exactly once by a single
/// template, which already evaluates once and in order.
fn bind_options(call: &mut Call, translation: &Translation, span: Span) -> Vec<Expr> {
    let option = |call: &Call, name: &str| match name {
        "count" => call.count.clone(),
        _ => call.values.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()),
    };
    let effectful: Vec<String> = call.order.iter().filter(|n| option(call, n).is_some_and(|v| !is_plain(&v))).cloned().collect();
    let unbound: Vec<String> = call
        .order
        .iter()
        .filter(|n| option(call, n).is_some_and(|v| !matches!(&*v.node, ExprNode::Lit { .. })))
        .cloned()
        .collect();
    if effectful.is_empty() {
        return Vec::new();
    }
    if let ([only], [_], Translation::One(template)) = (effectful.as_slice(), unbound.as_slice(), translation) {
        let reads = crate::i18n::pieces(template)
            .map(|p| p.iter().filter(|x| matches!(x, Piece::Name(n) if n == only)).count())
            .unwrap_or(0);
        if reads == 1 && call.lookup.interpolates() {
            return Vec::new();
        }
    }
    let mut assigns = Vec::new();
    for name in unbound {
        let local = Symbol::from(format!("__i18n_{name}_{}", span.start));
        let value = option_mut(call, &name).expect("collected above");
        let read = Expr { ty: value.ty.clone(), ..Expr::new(span, ExprNode::Var { id: VarId(0), name: local.clone() }) };
        let value = std::mem::replace(value, read);
        assigns.push(Expr::new(span, ExprNode::Assign { target: LValue::Var { id: VarId(0), name: local }, value }));
    }
    assigns
}

fn option_mut<'a>(call: &'a mut Call, name: &str) -> Option<&'a mut Expr> {
    match name {
        "count" => call.count.as_mut(),
        _ => call.values.iter_mut().find(|(n, _)| n == name).map(|(_, v)| v),
    }
}

/// A read that evaluates to the same value each time it is repeated.
fn is_plain(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Lit { .. })
}

/// `template` with each `%{name}` replaced by its value's `to_s`, or
/// as written when the call passes no value.
fn interpolate(span: Span, template: &str, call: &Call, html: bool) -> Expr {
    let pieces = match call.lookup.interpolates() {
        true => crate::i18n::pieces(template).unwrap_or_else(|_| vec![Piece::Text(template.to_string())]),
        false => vec![Piece::Text(template.to_string())],
    };
    let mut parts = Vec::new();
    for piece in pieces {
        match piece {
            Piece::Text(value) => parts.push(InterpPart::Text { value }),
            Piece::Name(name) => {
                let value = if name == "count" { call.count.clone() } else { call.values.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone()) };
                let value = value.expect("resolve refuses a name the call does not pass");
                parts.push(InterpPart::Expr { expr: if html { escaped(span, value) } else { value } });
            }
        }
    }
    let mut e = match parts.as_slice() {
        [] => Expr::new(span, ExprNode::Lit { value: Literal::Str { value: String::new() } }),
        [InterpPart::Text { value }] => Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.clone() } }),
        _ => Expr::new(span, ExprNode::StringInterp { parts }),
    };
    e.ty = Some(Ty::Str);
    e
}
