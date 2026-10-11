//! Rewritten at ingest, not in the lowering, so the analyzer types each
//! prop expression where it is written and the synthesized share methods
//! have callers during analysis.

use crate::App;
use crate::dialect::{Controller, ControllerBodyItem, Filter, FilterKind};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

use super::controller_macro_synth::append_private_actions;
use super::rate_limit::ruby_string_literal;
use super::{IngestError, survey};

#[derive(Clone, Copy, PartialEq)]
enum PropKind {
    Eager,
    Optional,
    Deferred,
}

struct Prop {
    key: String,
    kind: PropKind,
    group: String,
    value: Expr,
}

pub fn lower_inertia(app: &mut App) {
    let shares = expand_shares(app);
    let config = app.inertia.clone().unwrap_or_default();
    let mut uses_inertia = !shares.is_empty();
    let parents: Vec<(ClassId, Option<ClassId>)> =
        app.controllers.iter().map(|c| (c.name.clone(), c.parent.clone())).collect();
    for controller in &mut app.controllers {
        let mut chain = vec![controller.name.clone()];
        let mut cur = controller.parent.clone();
        while let Some(p) = cur {
            if chain.len() > 8 {
                break;
            }
            cur = parents.iter().find(|(n, _)| *n == p).and_then(|(_, pp)| pp.clone());
            chain.push(p);
        }
        let share_methods: Vec<Symbol> = chain
            .iter()
            .rev()
            .filter_map(|c| shares.iter().find(|(n, _)| n == c))
            .flat_map(|(_, methods)| methods.iter().cloned())
            .collect();
        let mut ctx = RenderCtx { config: &config, share_methods: &share_methods, used: false };
        for item in &mut controller.body {
            if let ControllerBodyItem::Action { action, .. } = item {
                desugar(&mut action.body, &mut ctx);
            }
        }
        uses_inertia |= ctx.used;
    }
    if uses_inertia {
        install_see_other(app);
    }
}

fn expand_shares(app: &mut App) -> Vec<(ClassId, Vec<Symbol>)> {
    let mut out = Vec::new();
    for controller in &mut app.controllers {
        let base = format!(
            "inertia_share_{}",
            crate::naming::snake_case(&controller.name.0.as_str().replace("::", ""))
        );
        let mut sources = String::new();
        let mut methods: Vec<Symbol> = Vec::new();
        let mut consumed: Vec<usize> = Vec::new();
        for (i, item) in controller.body.iter().enumerate() {
            let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            let Some(props) = share_props(expr) else { continue };
            let name = if methods.is_empty() { base.clone() } else { format!("{base}_{}", methods.len() + 1) };
            sources.push_str(&share_method_source(&name, &props));
            methods.push(Symbol::from(name.as_str()));
            consumed.push(i);
        }
        if methods.is_empty() || !append_private_actions(controller, "<inertia_share>", &sources) {
            continue;
        }
        for i in consumed.into_iter().rev() {
            controller.body.remove(i);
        }
        out.push((controller.name.clone(), methods));
    }
    out
}

fn share_props(call: &Expr) -> Option<Vec<Prop>> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*call.node else {
        return None;
    };
    if method.as_str() != "inertia_share" {
        return None;
    }
    let [hash] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, .. } = &*hash.node else { return None };
    let mut props = Vec::new();
    for (k, v) in entries {
        let key = literal_key(k)?;
        if matches!(key.as_str(), "if" | "unless" | "only" | "except") {
            return None;
        }
        let (kind, group, value) = match &*v.node {
            ExprNode::Lambda { params, rest_param: None, block_param: None, body, .. }
                if params.is_empty() =>
            {
                (PropKind::Eager, String::new(), body.clone())
            }
            _ => match prop_wrapper(v)? {
                Some((kind, group, body)) => (kind, group, body),
                None => (PropKind::Eager, String::new(), v.clone()),
            },
        };
        props.push(Prop { key, kind, group, value });
    }
    Some(props)
}

fn share_method_source(name: &str, props: &[Prop]) -> String {
    let mut src = format!("  def {name}\n");
    for p in props {
        let key = ruby_string_literal(&p.key);
        let guard = match p.kind {
            PropKind::Eager => format!("shared_eager?({key})"),
            PropKind::Optional => format!("shared_optional?({key})"),
            PropKind::Deferred => format!("shared_deferred?({key}, {})", ruby_string_literal(&p.group)),
        };
        let value = crate::emit::ruby::expr::emit_expr(&p.value);
        src.push_str(&format!(
            "    if inertia_page.{guard}\n      inertia_page.prop({key}, ({value}))\n    end\n"
        ));
    }
    src.push_str("  end\n");
    src
}

fn prop_wrapper(v: &Expr) -> Option<Option<(PropKind, String, Expr)>> {
    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*v.node else {
        return Some(None);
    };
    let ExprNode::Const { path } = &*recv.node else { return Some(None) };
    if path.len() != 1 || path[0].as_str() != "InertiaRails" {
        return Some(None);
    }
    let body = match block.as_ref().map(|b| &*b.node) {
        Some(ExprNode::Lambda { params, body, .. }) if params.is_empty() => body.clone(),
        _ => return None,
    };
    match method.as_str() {
        "optional" if args.is_empty() => Some(Some((PropKind::Optional, String::new(), body))),
        "defer" => {
            let mut group = "default".to_string();
            match args.as_slice() {
                [] => {}
                [opts] => {
                    let ExprNode::Hash { entries, .. } = &*opts.node else { return None };
                    for (k, gv) in entries {
                        if literal_key(k)?.as_str() != "group" {
                            return None;
                        }
                        let ExprNode::Lit { value: Literal::Str { value } } = &*gv.node else {
                            return None;
                        };
                        group = value.clone();
                    }
                }
                _ => return None,
            }
            Some(Some((PropKind::Deferred, group, body)))
        }
        _ => None,
    }
}

fn literal_key(k: &Expr) -> Option<String> {
    match &*k.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

struct RenderCtx<'a> {
    config: &'a crate::app::InertiaConfig,
    share_methods: &'a [Symbol],
    used: bool,
}

fn desugar(e: &mut Expr, ctx: &mut RenderCtx) {
    if let Some(replacement) = render_replacement(e, ctx).or_else(|| redirect_replacement(e)) {
        ctx.used = true;
        *e = replacement;
        return;
    }
    e.node.for_each_child_mut(&mut |c| desugar(c, ctx));
}

fn render_replacement(e: &Expr, ctx: &RenderCtx) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "render" {
        return None;
    }
    let [opts] = args.as_slice() else { return None };
    let ExprNode::Hash { entries, kwargs: true } = &*opts.node else { return None };
    let component = entries.iter().find(|(k, _)| literal_key(k).as_deref() == Some("inertia"))?;
    let span = e.span;
    let unsupported = |why: &str| {
        record_gap(span, &format!("`render inertia:` {why}; the call is left as written"));
        None
    };
    let ExprNode::Lit { value: Literal::Str { value: component } } = &*component.1.node else {
        return unsupported("with a component that is not a String literal");
    };
    let mut props: Vec<Prop> = Vec::new();
    let mut status: Option<Expr> = None;
    for (k, v) in entries {
        match literal_key(k).as_deref() {
            Some("inertia") => {}
            Some("status") => status = Some(v.clone()),
            Some("props") => {
                let ExprNode::Hash { entries: pairs, .. } = &*v.node else {
                    return unsupported("with `props:` that is not a Hash literal");
                };
                for (pk, pv) in pairs {
                    let Some(key) = literal_key(pk) else {
                        return unsupported("with a prop key that is not a literal");
                    };
                    let Some(wrapped) = prop_wrapper(pv) else {
                        return unsupported("with an InertiaRails prop type other than `optional`/`defer`");
                    };
                    let (kind, group, value) =
                        wrapped.unwrap_or((PropKind::Eager, String::new(), pv.clone()));
                    props.push(Prop { key, kind, group, value });
                }
            }
            _ => return unsupported("with an option other than `props:`/`status:`"),
        }
    }
    let status = status.unwrap_or_else(|| sym(span, "ok"));
    let config = ctx.config;
    let mut stmts = vec![call(
        span,
        None,
        "inertia_begin",
        vec![
            str_lit(span, component),
            str_lit(span, &config.version),
            bool_lit(span, config.encrypt_history),
            bool_lit(span, config.always_include_errors_hash),
        ],
    )];
    for m in ctx.share_methods {
        stmts.push(call(span, None, m.as_str(), Vec::new()));
    }
    for p in props {
        let page = || call(span, None, "inertia_page", Vec::new());
        let key = str_lit(span, &p.key);
        let guard = match p.kind {
            PropKind::Eager => call(span, Some(page()), "eager?", vec![key.clone()]),
            PropKind::Optional => call(span, Some(page()), "optional?", vec![key.clone()]),
            PropKind::Deferred => {
                call(span, Some(page()), "deferred?", vec![key.clone(), str_lit(span, &p.group)])
            }
        };
        let set = call(span, Some(page()), "prop", vec![key, p.value]);
        stmts.push(if_expr(span, guard, set, nil(span)));
    }
    let status_kw = |s: &Expr| (sym(span, "status"), s.clone());
    let json = call(span, None, "render_inertia_json", vec![kwargs(span, vec![status_kw(&status)])]);
    let html = call(
        span,
        None,
        "render",
        vec![
            call(span, None, "inertia_root_html", vec![bool_lit(span, config.use_script_element_for_initial_page)]),
            kwargs(span, vec![(sym(span, "layout"), str_lit(span, "application")), status_kw(&status)]),
        ],
    );
    stmts.push(if_expr(span, call(span, None, "inertia_request?", Vec::new()), json, html));
    Some(Expr::new(span, ExprNode::Seq { exprs: stmts }))
}

fn redirect_replacement(e: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block: None, parenthesized } = &*e.node else {
        return None;
    };
    if !matches!(method.as_str(), "redirect_to" | "redirect_back_or_to") {
        return None;
    }
    let (opts, rest) = args.split_last()?;
    let ExprNode::Hash { entries, kwargs: true } = &*opts.node else { return None };
    let (_, inertia) = entries.iter().find(|(k, _)| literal_key(k).as_deref() == Some("inertia"))?;
    let ExprNode::Hash { entries: inertia_entries, .. } = &*inertia.node else {
        record_gap(e.span, "`inertia:` redirect option that is not a Hash literal; the call is left as written");
        return None;
    };
    let mut errors = None;
    for (k, v) in inertia_entries {
        if literal_key(k).as_deref() != Some("errors") {
            record_gap(e.span, "`inertia:` redirect option other than `errors:`; the call is left as written");
            return None;
        }
        errors = Some(v.clone());
    }
    let span = e.span;
    let mut stmts = Vec::new();
    if let Some(errors) = errors {
        // `to_hash`, not the value itself, because the gem stores
        // `errors.to_hash`; a Hash literal is passed as written.
        let errors = if matches!(&*errors.node, ExprNode::Hash { .. }) {
            errors
        } else {
            call(errors.span, Some(errors), "to_hash", Vec::new())
        };
        stmts.push(call(span, None, "inertia_errors", vec![errors]));
    }
    let kept: Vec<(Expr, Expr)> = entries
        .iter()
        .filter(|(k, _)| literal_key(k).as_deref() != Some("inertia"))
        .cloned()
        .collect();
    let mut new_args = rest.to_vec();
    if !kept.is_empty() {
        new_args.push(kwargs(span, kept));
    }
    stmts.push(Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: method.clone(),
            args: new_args,
            block: None,
            parenthesized: *parenthesized,
        },
    ));
    Some(Expr::new(span, ExprNode::Seq { exprs: stmts }))
}

/// An `after_action` on the root controllers rather than a change in each
/// dispatcher (the two scaffold `main.rb` files and the test harness),
/// which is where inertia_rails' middleware would sit.
fn install_see_other(app: &mut App) {
    let names: Vec<ClassId> = app.controllers.iter().map(|c| c.name.clone()).collect();
    for controller in &mut app.controllers {
        let is_root = controller.parent.as_ref().is_none_or(|p| !names.contains(p));
        if !is_root {
            continue;
        }
        let source = "  def inertia_see_other\n    inertia_redirect_see_other\n  end\n";
        if !append_private_actions(controller, "<inertia_see_other>", source) {
            continue;
        }
        push_after_filter(controller, "inertia_see_other");
    }
}

fn push_after_filter(controller: &mut Controller, target: &str) {
    let filter = Filter {
        target_span: Span::synthetic(),
        kind: FilterKind::After,
        target: Symbol::from(target),
        from_concern: None,
        only: Vec::new(),
        except: Vec::new(),
        only_style: Default::default(),
        except_style: Default::default(),
        if_cond: None,
        unless_cond: None,
        if_cond_expr: None,
        unless_cond_expr: None,
        block: None,
        prepend: false,
    };
    controller.body.insert(
        0,
        ControllerBodyItem::Filter { filter, leading_comments: Vec::new(), leading_blank_line: false },
    );
}

fn record_gap(span: Span, message: &str) {
    let file = super::sources::path_of(span.file).unwrap_or_default();
    survey::record(&IngestError::Unsupported { file, message: message.to_string() });
}

fn call(span: Span, recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true },
    )
}

fn if_expr(span: Span, cond: Expr, then_branch: Expr, else_branch: Expr) -> Expr {
    Expr::new(span, ExprNode::If { cond, then_branch, else_branch })
}

fn kwargs(span: Span, entries: Vec<(Expr, Expr)>) -> Expr {
    Expr::new(span, ExprNode::Hash { entries, kwargs: true })
}

fn str_lit(span: Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
}

fn bool_lit(span: Span, value: bool) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Bool { value } })
}

fn sym(span: Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
}

fn nil(span: Span) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Nil })
}
