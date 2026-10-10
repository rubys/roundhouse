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
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::i18n::{Catalog, Fallback, Translate, Translation};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

/// A translate call's lookup, and the expressions its `count:` and
/// interpolation values read.
pub(crate) struct Call {
    pub lookup: Translate,
    count: Option<Expr>,
    values: Vec<(String, Expr)>,
}

/// `None` when this is not a translate call; `Err` when it is one whose
/// shape cannot be resolved at compile time.
pub(crate) fn parse(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
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
            }
            "locale" | "raise" | "throw" | "separator" | "exception_handler" | "fallback" => {
                return Err(format!("the `{name}:` option"));
            }
            _ => {
                call.lookup.values.push(name.as_str().to_string());
                call.values.push((name.as_str().to_string(), v.clone()));
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
    let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else { return };
    let Some(Ok(call)) = parse(recv.as_ref(), method.as_str(), args, in_view, view) else { return };
    let Ok(translation) = catalog.resolve(&call.lookup) else { return };
    let span = expr.span;
    let html = in_view && crate::i18n::is_html_safe_key(&call.lookup.key);
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

/// `template` with each `%{name}` replaced by its value's `to_s`.
fn interpolate(span: Span, template: &str, call: &Call, html: bool) -> Expr {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(i) = rest.find("%{") {
        let Some(j) = rest[i + 2..].find('}') else { break };
        let name = &rest[i + 2..i + 2 + j];
        let value = if name == "count" {
            call.count.clone()
        } else {
            call.values.iter().find(|(n, _)| n == name).map(|(_, v)| if html { escaped(span, v.clone()) } else { v.clone() })
        };
        let Some(value) = value else { break };
        if i > 0 {
            parts.push(InterpPart::Text { value: rest[..i].to_string() });
        }
        parts.push(InterpPart::Expr { expr: value });
        rest = &rest[i + 3 + j..];
    }
    if !rest.is_empty() {
        parts.push(InterpPart::Text { value: rest.to_string() });
    }
    let mut e = match parts.as_slice() {
        [] => Expr::new(span, ExprNode::Lit { value: Literal::Str { value: String::new() } }),
        [InterpPart::Text { value }] => Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.clone() } }),
        _ => Expr::new(span, ExprNode::StringInterp { parts }),
    };
    e.ty = Some(Ty::Str);
    e
}
