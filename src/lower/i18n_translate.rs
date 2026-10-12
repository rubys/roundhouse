//! `t("key", …)` in a template and `I18n.t("key", …)` anywhere → the
//! String Rails' I18n answers from the app's locale (`crate::i18n`).
//!
//! The lookup happens at compile time: a literal key (or a view's lazy
//! `.key`), literal `scope:` / `default:`, and interpolation values that
//! are any expression. `count:` keeps the choice between plural forms
//! for run time. A template's `_html` key renders unescaped with its
//! values escaped, as `HtmlSafeTranslation` does. A key the locale lacks
//! answers as Rails does in the running environment
//! (`raise_on_missing_translations`, read per environment at ingest).
//! Anything else (a dynamic key, `locale:`, a missing key with only key
//! defaults, a setting not readable at compile time) is an error in
//! analyze; only what `parse` and `Catalog::resolve` accept is typed,
//! and only that folds.

use crate::app::App;
use crate::expr::{Arm, Expr, ExprNode, InterpPart, LValue, Literal, Pattern};
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
    /// A key with run-time text: its parts after the literal prefix
    /// (`lookup.key`), and the literal text it ends in.
    dynamic: Option<(Vec<InterpPart>, String)>,
}

/// Most translations one run-time key may reach before folding each
/// stops being worth the branch.
const MAX_REACH: usize = 400;

/// How a translate call folds: one translation, or one per key a
/// run-time key can reach, the subtrees it can name, and what it answers
/// for any other key.
pub(crate) enum Plan {
    Static(Translation),
    Dynamic { arms: Vec<(String, Translation)>, subtrees: Vec<String>, otherwise: Translation, otherwise_html: bool },
}

pub(crate) fn plan(catalog: &Catalog, call: &Call, in_view: bool) -> Result<Plan, String> {
    let Some((_, tail)) = &call.dynamic else {
        return catalog.resolve(&call.lookup).map(Plan::Static);
    };
    let base = Translate::full_key(&call.lookup, &call.lookup.key)?;
    let (keys, subtrees) = catalog.reach(&base, tail, call.lookup.counted);
    if keys.len() + subtrees.len() > MAX_REACH {
        return Err(format!("a key computed at run time that can reach {} translations", keys.len() + subtrees.len()));
    }
    let mut arms = Vec::new();
    for key in keys {
        let lookup = Translate { key: key.clone(), view: None, scope: None, defaults: Vec::new(), ..call.lookup.clone() };
        arms.push((key, catalog.resolve(&lookup)?));
    }
    let otherwise = if call.lookup.defaults.is_empty() {
        if let Some(why) = &catalog.missing.unknown {
            return Err(format!("a key computed at run time can be missing ({why})"));
        }
        Translation::Missing { key: String::new() }
    } else {
        catalog.resolve(&Translate { key: "\u{0}".into(), ..call.lookup.clone() })?
    };
    // A found translation is html-safe by the key the call passed, which only its literal tail can tell.
    let otherwise_html = crate::i18n::is_html_safe_key(tail);
    if in_view && !call.lookup.defaults.is_empty() && !otherwise_html && tail.trim_matches('.').is_empty() {
        return Err("a default under a key whose end is computed at run time, which decides whether it is html-safe".into());
    }
    Ok(Plan::Dynamic { arms, subtrees, otherwise, otherwise_html })
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
        ExprNode::StringInterp { parts } => parts
            .iter()
            .map(|p| match p {
                InterpPart::Text { value } => Some(value.clone()),
                InterpPart::Expr { expr } => interpolated_text(expr),
            })
            .collect(),
        _ => None,
    }
}

/// What `#{expr}` renders for a literal `expr`.
fn interpolated_text(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => Some(String::new()),
        ExprNode::Lit { value: Literal::Int { value } } => Some(value.to_string()),
        _ => literal_text(e),
    }
}

/// `"prefix.#{x}.tail"`: the literal text before the first run-time
/// part (which must not be empty), the parts from there on, and the
/// literal text after the last run-time part.
fn dynamic_key(key: &Expr) -> Option<(String, Vec<InterpPart>, String)> {
    let ExprNode::StringInterp { parts } = &*key.node else { return None };
    let literal = |p: &InterpPart| match p {
        InterpPart::Text { value } => Some(value.clone()),
        InterpPart::Expr { expr } => interpolated_text(expr),
    };
    let first = parts.iter().position(|p| literal(p).is_none())?;
    let last = parts.iter().rposition(|p| literal(p).is_none())?;
    let prefix: String = parts[..first].iter().filter_map(literal).collect();
    let tail: String = parts[last + 1..].iter().filter_map(literal).collect();
    (!prefix.is_empty()).then(|| (prefix, parts[first..].to_vec(), tail))
}

/// A key that picks between literals by a condition (`c ? "a" : "b"`,
/// or `"x.#{c ? 'a' : 'b'}"`): the condition, and the arguments with
/// the key each way it can go.
pub(crate) fn split_key(args: &[Expr]) -> Option<(Expr, Vec<Expr>, Vec<Expr>)> {
    let key = args.first()?;
    let with_key = |k: Expr| std::iter::once(k).chain(args[1..].iter().cloned()).collect::<Vec<_>>();
    let (cond, then_key, else_key) = match &*key.node {
        ExprNode::If { cond, then_branch, else_branch } => (cond.clone(), then_branch.clone(), else_branch.clone()),
        ExprNode::StringInterp { parts } => {
            let at = parts.iter().position(|p| match p {
                InterpPart::Expr { expr } => interpolated_text(expr).is_none(),
                InterpPart::Text { .. } => false,
            })?;
            let InterpPart::Expr { expr } = &parts[at] else { unreachable!() };
            let ExprNode::If { cond, then_branch, else_branch } = &*expr.node else { return None };
            let swap = |branch: &Expr| {
                let mut parts = parts.clone();
                parts[at] = InterpPart::Expr { expr: branch.clone() };
                Expr::new(key.span, ExprNode::StringInterp { parts })
            };
            (cond.clone(), swap(then_branch), swap(else_branch))
        }
        _ => return None,
    };
    Some((cond, with_key(then_key), with_key(else_key)))
}

/// Whether every key a translate call can look up resolves; `None` when
/// this is not a translate call.
pub(crate) fn resolves(
    catalog: &Catalog,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    in_view: bool,
    view: Option<&str>,
) -> Option<Result<(), String>> {
    let parsed = parse(recv, method, args, block, in_view, view)?;
    Some(match (parsed, split_key(args)) {
        (Err(reason), _) if block.is_some() => Err(reason),
        (_, Some((_, then_args, else_args))) => [then_args, else_args]
            .iter()
            .try_for_each(|a| resolves(catalog, recv, method, a, block, in_view, view).expect("still a translate call")),
        (Ok(call), None) => plan(catalog, &call, in_view).map(|_| ()),
        (Err(reason), None) => Err(reason),
    })
}

fn parse_args(args: &[Expr], view: Option<&str>) -> Result<Call, String> {
    let (key, opts) = match args {
        [key] => (key, None),
        [key, opts] => (key, Some(opts)),
        _ => return Err("an argument shape other than (key, **options)".into()),
    };
    let (key, dynamic) = match literal_text(key) {
        Some(key) => (key, None),
        None => {
            let (prefix, rest, tail) = dynamic_key(key).ok_or("a key computed at run time")?;
            (prefix, Some((rest, tail)))
        }
    };
    let mut call = Call {
        lookup: Translate { key, view: view.map(str::to_string), ..Default::default() },
        count: None,
        values: Vec::new(),
        order: Vec::new(),
        dynamic,
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
    let Some(parsed) = parse(recv.as_ref(), method.as_str(), args, block.as_ref(), in_view, view) else { return };
    if let Some((cond, then_args, else_args)) = split_key(args) {
        let resolved = resolves(catalog, recv.as_ref(), method.as_str(), args, block.as_ref(), in_view, view);
        if !matches!(resolved, Some(Ok(()))) {
            return;
        }
        let branch = |args: Vec<Expr>| {
            let mut e = expr.clone();
            if let ExprNode::Send { args: a, .. } = &mut *e.node {
                *a = args;
            }
            rewrite(&mut e, catalog, in_view, view);
            e
        };
        let (then_branch, else_branch) = (branch(then_args), branch(else_args));
        *expr = typed(Expr::new(expr.span, ExprNode::If { cond, then_branch, else_branch }));
        return;
    }
    let Ok(call) = parsed else { return };
    let Ok(plan) = plan(catalog, &call, in_view) else { return };
    let span = expr.span;
    *expr = match plan {
        Plan::Static(translation) => {
            let key = lit(span, format!("{}.{}", catalog.locale, Translate::full_key(&call.lookup, &call.lookup.key).unwrap_or_default()));
            let Some((folded, html)) = fold(call, translation, span, in_view, catalog, key) else { return };
            if html { raw(span, folded) } else { folded }
        }
        Plan::Dynamic { arms, subtrees, otherwise, otherwise_html } => {
            let Some(dynamic) = dynamic_fold(&call, arms, subtrees, otherwise, otherwise_html, span, in_view, catalog) else { return };
            dynamic
        }
    };
}

/// `__k = I18n.normalize_key("<scoped prefix>#{…}")`, then a `case` over
/// every key it can reach, each arm folded as that literal key. In a
/// template every arm is escaped or html-safe already, and the whole is
/// marked raw, since arms differ in which they are.
#[allow(clippy::too_many_arguments)]
fn dynamic_fold(
    call: &Call,
    arms: Vec<(String, Translation)>,
    subtrees: Vec<String>,
    otherwise: Translation,
    otherwise_html: bool,
    span: Span,
    in_view: bool,
    catalog: &Catalog,
) -> Option<Expr> {
    let (parts, _) = call.dynamic.as_ref()?;
    let base = Translate::full_key(&call.lookup, &call.lookup.key).ok()?;
    let local = Symbol::from(format!("__i18n_key_{}", span.start));
    let read = || typed(Expr::new(span, ExprNode::Var { id: VarId(0), name: local.clone() }));
    let key_text = typed(Expr::new(span, ExprNode::StringInterp {
        parts: std::iter::once(InterpPart::Text { value: base }).chain(parts.iter().cloned()).collect(),
    }));
    let assign = Expr::new(span, ExprNode::Assign {
        target: LValue::Var { id: VarId(0), name: local.clone() },
        value: typed(const_send(span, "I18n", "normalize_key", vec![key_text])),
    });
    let shown = |(folded, html): (Expr, bool)| match (in_view, html) {
        (true, false) => typed(Expr::new(span, ExprNode::Send {
            recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")] })),
            method: Symbol::from("html_escape"),
            args: vec![folded],
            block: None,
            parenthesized: true,
        })),
        _ => folded,
    };
    let mut case_arms = Vec::new();
    for (key, translation) in arms {
        let mut each = call.clone_for(&key);
        each.dynamic = None;
        let full = lit(span, format!("{}.{key}", catalog.locale));
        let body = shown(fold(each, translation, span, in_view, catalog, full)?);
        case_arms.push(Arm { pattern: Pattern::Lit { value: Literal::Str { value: key } }, guard: None, body });
    }
    for key in subtrees {
        let body = typed(const_send(span, "I18n", "subtree_translation", vec![read()]));
        case_arms.push(Arm { pattern: Pattern::Lit { value: Literal::Str { value: key } }, guard: None, body });
    }
    let missing_key = typed(Expr::new(span, ExprNode::StringInterp {
        parts: vec![InterpPart::Text { value: format!("{}.", catalog.locale) }, InterpPart::Expr { expr: read() }],
    }));
    let mut rest = call.clone_for(&call.lookup.key);
    rest.dynamic = None;
    let (folded, html) = fold(rest, otherwise, span, in_view, catalog, missing_key)?;
    let html = html || otherwise_html;
    case_arms.push(Arm { pattern: Pattern::Wildcard, guard: None, body: shown((folded, html)) });
    let case = typed(Expr::new(span, ExprNode::Case { scrutinee: read(), arms: case_arms }));
    let whole = typed(Expr::new(span, ExprNode::Seq { exprs: vec![assign, case] }));
    Some(if in_view { raw(span, whole) } else { whole })
}

impl Call {
    fn clone_for(&self, key: &str) -> Call {
        Call {
            lookup: Translate { key: key.to_string(), ..self.lookup.clone() },
            count: self.count.clone(),
            values: self.values.clone(),
            order: self.order.clone(),
            dynamic: self.dynamic.clone(),
        }
    }
}

/// `translation` as the call's value, with whether it is html-safe (an
/// `_html` key's, or a template's missing-translation span). `key` is
/// the missing key's text, read only for `Translation::Missing`.
fn fold(mut call: Call, translation: Translation, span: Span, in_view: bool, catalog: &Catalog, key: Expr) -> Option<(Expr, bool)> {
    let html = in_view && (crate::i18n::is_html_safe_key(&call.lookup.key) || matches!(translation, Translation::Missing { .. }));
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
        (Translation::Plural { .. }, None) => return None,
        (Translation::Missing { .. }, _) => missing(span, key, &call, in_view, &catalog.missing),
    };
    folded.ty = Some(Ty::Str);
    if !bound.is_empty() {
        folded = typed(Expr::new(span, ExprNode::Seq { exprs: bound.into_iter().chain([folded]).collect() }));
    }
    Some((folded, html))
}

fn lit(span: Span, value: String) -> Expr {
    typed(Expr::new(span, ExprNode::Lit { value: Literal::Str { value } }))
}

fn const_send(span: Span, recv: &str, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(span, ExprNode::Send {
        recv: Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from(recv)] })),
        method: Symbol::from(method),
        args,
        block: None,
        parenthesized: true,
    })
}

fn raw(span: Span, folded: Expr) -> Expr {
    typed(Expr::new(span, ExprNode::Send {
        recv: None,
        method: Symbol::from("raw"),
        args: vec![folded],
        block: None,
        parenthesized: true,
    }))
}

/// `I18n.missing_translation(key, raises)`, or in a template the debug
/// span Action View renders, titled with the call's options in order.
fn missing(span: Span, key: Expr, call: &Call, in_view: bool, policy: &crate::i18n::MissingPolicy) -> Expr {
    let mut raises = const_send(span, "I18n", "missing_raises", vec![
        Expr::new(span, ExprNode::Lit { value: Literal::Bool { value: policy.raises } }),
        lit(span, policy.flipped.join(",")),
    ]);
    raises.ty = Some(Ty::Bool);
    if !in_view {
        return typed(const_send(span, "I18n", "missing_translation", vec![key, raises]));
    }
    let mut parts = Vec::new();
    for name in &call.order {
        let value = match name.as_str() {
            "count" => call.count.clone(),
            _ => call.values.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()),
        };
        if let Some(value) = value {
            parts.push(InterpPart::Text { value: format!(", {name}: ") });
            parts.push(InterpPart::Expr { expr: escaped(span, value) });
        }
    }
    let details = if parts.is_empty() { lit(span, String::new()) } else { typed(Expr::new(span, ExprNode::StringInterp { parts })) };
    typed(const_send(span, "I18n", "missing_translation_tag", vec![key, details, raises]))
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
