//! A mailer action's `mail(...)` renders its templates, as ActionMailer's
//! does: `<mailer>/<action>.text.*` and `<mailer>/<action>.html.*`, each
//! wrapped in the mailer's layout of that format, become the message's
//! parts (both: multipart/alternative; one: the body). The class body's
//! `default from: …` fills the headers an action leaves out, and its
//! `layout "mailer"` names the layout; both are read up the mailer chain,
//! nearest first, here at compile time.
//!
//!   mail(to: user.email, subject: "hi")
//!   # becomes
//!   mail({ from: "from@example.com", to: user.email, subject: "hi" },
//!        Views::Layouts.mailer_text(Views::UserMailer.welcome_text(@user), nil, nil),
//!        Views::Layouts.mailer(Views::UserMailer.welcome(@user), nil, nil))
//!
//! What it does not model is reported rather than sent empty: a `mail`
//! with a `format` block, `template_name:` / `template_path:`, an action
//! with no template and no `body:`, and a `default` given a Proc.

use std::collections::{BTreeSet, HashMap};

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::span::Span;
use crate::ty::Ty;

use super::view_to_library::{ViewArgs, ViewKey};

pub fn apply_mailer_templates(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let mailers = super::mailer_class_side::mailer_names(app);
    if mailers.is_empty() {
        return diags;
    }
    let contracts = super::view_to_library::action_view_ivar_map(&app.views, &app.controllers, &app.models);
    let closures = super::view_to_library::view_ivar_closures(&app.views, &app.controllers);
    let parents: HashMap<String, Option<String>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str().to_string(), lc.parent.as_ref().map(|p| p.0.as_str().to_string())))
        .collect();
    let calls: HashMap<String, Vec<Expr>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str().to_string(), lc.unknown_calls.clone()))
        .collect();
    let views: Vec<(String, String, bool)> = app
        .views
        .iter()
        .filter(|v| crate::lower::view::lowers_through_view_path(v))
        .map(|v| (v.name.as_str().to_string(), v.format.as_str().to_string(), super::view_to_library::view_uses_bare_name(&v.body, "flash")))
        .collect();
    let has_view = |name: &str, format: &str| views.iter().any(|(n, f, _)| n == name && f == format);

    for lc in app.library_classes.iter_mut() {
        let class = lc.name.0.as_str().to_string();
        if !mailers.contains(&class) {
            continue;
        }
        let chain = ancestry(&class, &parents, &mailers);
        let (defaults, layout) = class_settings(&chain, &calls, &mut diags);
        let layout = layout.unwrap_or_else(|| implied_layout(&chain, &has_view));
        let dir = view_dir(&class);
        let module = crate::naming::camelize_path(&dir);
        for m in lc.methods.iter_mut() {
            if m.receiver != crate::dialect::MethodReceiver::Instance || m.name.as_str() == "initialize" {
                continue;
            }
            let action = m.name.as_str().to_string();
            let template = format!("{dir}/{action}");
            let site = Site {
                module: &module,
                dir: &dir,
                action: &action,
                text: has_view(&template, "text"),
                html: has_view(&template, "html"),
                layout: layout.as_deref(),
                contracts: &contracts,
                closures: &closures,
                views: &views,
                defaults: &defaults,
            };
            rewrite(&mut m.body, &site, &mut diags);
        }
    }
    diags
}

struct Site<'a> {
    module: &'a str,
    dir: &'a str,
    action: &'a str,
    text: bool,
    html: bool,
    layout: Option<&'a str>,
    contracts: &'a HashMap<(String, String), ViewArgs>,
    closures: &'a HashMap<ViewKey, Vec<Symbol>>,
    views: &'a [(String, String, bool)],
    defaults: &'a [(Expr, Expr)],
}

/// The mailer and its mailer ancestors, nearest first.
fn ancestry(class: &str, parents: &HashMap<String, Option<String>>, mailers: &BTreeSet<String>) -> Vec<String> {
    let mut out = vec![class.to_string()];
    let mut current = class.to_string();
    while let Some(Some(parent)) = parents.get(&current) {
        if !mailers.contains(parent) || out.contains(parent) {
            break;
        }
        out.push(parent.clone());
        current = parent.clone();
    }
    out
}

/// `default k: v` entries (a nearer class's key wins) and the nearest
/// `layout` (`Some(None)` for `layout false`).
fn class_settings(
    chain: &[String],
    calls: &HashMap<String, Vec<Expr>>,
    diags: &mut Vec<Diagnostic>,
) -> (Vec<(Expr, Expr)>, Option<Option<String>>) {
    let mut defaults: Vec<(Expr, Expr)> = Vec::new();
    let mut layout: Option<Option<String>> = None;
    for class in chain {
        for call in calls.get(class).into_iter().flatten() {
            let ExprNode::Send { recv: None, method, args, .. } = &*call.node else { continue };
            match (method.as_str(), args.as_slice()) {
                ("default", [hash]) => {
                    let ExprNode::Hash { entries, .. } = &*hash.node else { continue };
                    for (k, v) in entries {
                        if matches!(&*v.node, ExprNode::Lambda { .. }) {
                            diags.push(Diagnostic::unsupported(v.span, None, "mailer default", "a `default` given a Proc runs per message in Rails; it is not evaluated here"));
                            continue;
                        }
                        if !defaults.iter().any(|(dk, _)| same_key(dk, k)) {
                            defaults.push((k.clone(), v.clone()));
                        }
                    }
                }
                ("layout", [name]) if layout.is_none() => {
                    layout = Some(match &*name.node {
                        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
                        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                        _ => None,
                    });
                }
                _ => {}
            }
        }
    }
    (defaults, layout)
}

/// With no `layout` declared, ActionMailer looks for one named after the
/// mailer, then after each ancestor.
fn implied_layout(chain: &[String], has_view: &dyn Fn(&str, &str) -> bool) -> Option<String> {
    chain.iter().map(|c| view_dir(c)).find(|stem| {
        has_view(&format!("layouts/{stem}"), "html") || has_view(&format!("layouts/{stem}"), "text")
    })
}

/// `Admin::UserMailer` → `admin/user_mailer`, its templates' directory.
fn view_dir(class: &str) -> String {
    class.split("::").map(crate::naming::snake_case).collect::<Vec<_>>().join("/")
}

fn same_key(a: &Expr, b: &Expr) -> bool {
    let name = |e: &Expr| match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    };
    name(a).is_some() && name(a) == name(b)
}

fn rewrite(expr: &mut Expr, site: &Site<'_>, diags: &mut Vec<Diagnostic>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, site, diags));
    let ExprNode::Send { recv: None, method, args, block, parenthesized } = &mut *expr.node else { return };
    if method.as_str() != "mail" || args.len() > 1 {
        return;
    }
    let span = expr.span;
    if block.is_some() {
        diags.push(Diagnostic::unsupported(span, None, "mailer format block", "`mail { |format| … }` chooses the parts itself; only the templates' own parts are rendered"));
        return;
    }
    let opts = args.first().cloned();
    let opt_keys: Vec<String> = match opts.as_ref().map(|o| &*o.node) {
        Some(ExprNode::Hash { entries, .. }) => entries
            .iter()
            .filter_map(|(k, _)| match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    if opt_keys.iter().any(|k| k == "template_name" || k == "template_path") {
        diags.push(Diagnostic::unsupported(span, None, "mailer template option", "`template_name:` / `template_path:` choose another template, which is not modeled"));
        return;
    }
    let given_body = opt_keys.iter().any(|k| k == "body");
    if !given_body && !site.text && !site.html {
        diags.push(Diagnostic::unsupported(
            span,
            None,
            "mailer template",
            format!("no `{}/{}` template: ActionMailer raises ActionView::MissingTemplate", site.dir, site.action),
        ));
        return;
    }
    let part = |format: &str, present: bool| -> Expr {
        if given_body || !present {
            return nil(span);
        }
        let inner = view_call(site, format, span);
        match site.layout {
            Some(layout) => layout_call(site, layout, format, inner, span),
            None => inner,
        }
    };
    let text = part("text", site.text);
    let html = part("html", site.html);
    let merged = merged_opts(opts, site.defaults, span);
    *args = vec![merged, text, html];
    // `mail from: …` re-spelled bare would open with `{`, which Ruby reads as a block.
    *parenthesized = true;
}

fn view_call(site: &Site<'_>, format: &str, span: Span) -> Expr {
    let stem = if format == "html" { site.action.to_string() } else { format!("{}_{format}", site.action) };
    let contract = site.contracts.get(&(site.module.to_string(), stem.clone()));
    let mut args: Vec<Expr> = contract.map(|c| c.ivars.iter().map(|n| ivar(n, span)).collect()).unwrap_or_default();
    if let Some(c) = contract
        && (c.uses_action_name || c.uses_controller_name || c.uses_path_parameters)
    {
        args.push(nil(span));
        args.push(nil(span));
        if c.uses_action_name {
            args.push(str_lit(span, site.action));
        }
        if c.uses_controller_name {
            args.push(str_lit(span, site.dir));
        }
        if c.uses_path_parameters {
            args.push(nil(span));
        }
    }
    let method = crate::lower::view::view_method_name_for(site.action, format);
    typed(send(Some(views_const(&[site.module], span)), method.as_str(), args, span), Ty::Str)
}

/// The layout of `format` wrapping `inner`, as the controller's layout
/// wrap builds it, with nil where a controller passes its flash: a
/// mailer has none.
fn layout_call(site: &Site<'_>, layout: &str, format: &str, inner: Expr, span: Span) -> Expr {
    let name = format!("layouts/{layout}");
    let Some((_, _, uses_flash)) = site.views.iter().find(|(n, f, _)| *n == name && f == format) else {
        return inner;
    };
    let key: ViewKey = ("Layouts".to_string(), layout.to_string());
    let mut args = vec![inner];
    for iv in site.closures.get(&key).into_iter().flatten() {
        args.push(ivar(iv, span));
    }
    if *uses_flash {
        args.push(nil(span));
    }
    args.push(nil(span));
    args.push(nil(span));
    let method = crate::lower::view::view_method_name_for(layout, format);
    typed(send(Some(views_const(&["Layouts"], span)), method.as_str(), args, span), Ty::Str)
}

/// The class's defaults under the action's own headers.
fn merged_opts(opts: Option<Expr>, defaults: &[(Expr, Expr)], span: Span) -> Expr {
    let hash_ty = Ty::Hash { key: std::sync::Arc::new(Ty::Sym), value: std::sync::Arc::new(Ty::Untyped) };
    let hash = |entries: Vec<(Expr, Expr)>| typed(Expr::new(span, ExprNode::Hash { entries, kwargs: false }), hash_ty.clone());
    match opts {
        Some(o) => match &*o.node {
            ExprNode::Hash { entries, .. } => {
                let mut out: Vec<(Expr, Expr)> =
                    defaults.iter().filter(|(dk, _)| !entries.iter().any(|(k, _)| same_key(k, dk))).cloned().collect();
                out.extend(entries.iter().cloned());
                hash(out)
            }
            _ if defaults.is_empty() => o,
            _ => typed(send(Some(hash(defaults.to_vec())), "merge", vec![o], span), hash_ty.clone()),
        },
        None => hash(defaults.to_vec()),
    }
}

fn views_const(rest: &[&str], span: Span) -> Expr {
    let mut path = vec![Symbol::from("Views")];
    path.extend(rest.iter().filter(|s| !s.is_empty()).map(|s| Symbol::from(*s)));
    Expr::new(span, ExprNode::Const { path })
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>, span: Span) -> Expr {
    Expr::new(span, ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true })
}

fn ivar(name: &Symbol, span: Span) -> Expr {
    Expr::new(span, ExprNode::Ivar { name: name.clone() })
}

fn nil(span: Span) -> Expr {
    typed(Expr::new(span, ExprNode::Lit { value: Literal::Nil }), Ty::Nil)
}

fn str_lit(span: Span, value: &str) -> Expr {
    typed(Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } }), Ty::Str)
}

fn typed(mut e: Expr, ty: Ty) -> Expr {
    e.ty = Some(ty);
    e
}
