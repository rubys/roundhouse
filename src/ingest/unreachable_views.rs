//! A template no request can reach has its errors reported as warnings:
//! its ivars have no action to come from, so every read reported "has
//! no known type" against a page Rails never renders (lobsters'
//! `mod/notes/index`, whose route redirects away and whose controller
//! has only `create`). It is still typed and emitted, so a judgement
//! made wrongly here changes what is reported, not what runs.
//!
//! Unreachable means all of: its directory's controller exists and
//! descends from Rails' own base through the app's controllers; neither
//! that controller, its ancestors, the modules they include, nor any
//! controller inheriting from it (which renders an inherited template)
//! defines the action; no route names it; and no `render` in those
//! controllers or anywhere else names the template. A controller among
//! them that renders a computed name, or includes a module the app does
//! not define, keeps every template of the directory, since what it
//! renders cannot be read here.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::{Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, Literal};

pub(super) fn mark(app: &mut App) {
    if app.controllers.is_empty() || app.views.is_empty() {
        return;
    }
    let by_name: HashMap<&str, &Controller> = app.controllers.iter().map(|c| (c.name.0.as_str(), c)).collect();
    let modules: HashMap<&str, HashSet<&str>> = app
        .library_classes
        .iter()
        .map(|lc| (lc.name.0.as_str(), lc.methods.iter().map(|m| m.name.as_str()).collect()))
        .collect();
    let routes: HashSet<(String, String)> = crate::lower::routes::flatten_routes_for_helpers(app)
        .into_iter()
        .map(|r| (r.controller.0.as_str().to_string(), r.action.as_str().to_string()))
        .collect();
    let named_anywhere = render_literals_outside_controllers(app);

    let mut unreachable: Vec<usize> = Vec::new();
    for (i, view) in app.views.iter().enumerate() {
        let (dir, base) = crate::lower::view_to_library::split_view_name(view.name.as_str());
        if dir.is_empty() || dir.starts_with("layouts") || base.starts_with('_') {
            continue;
        }
        let class = format!("{}Controller", crate::naming::camelize_path(dir));
        let Some(controller) = by_name.get(class.as_str()) else { continue };
        if !rooted_in_rails(controller, &by_name) {
            continue;
        }
        let family = family(controller, &by_name);
        let reachable = family.iter().any(|c| {
            defines(c, base, &modules)
                || routes.contains(&(c.name.0.as_str().to_string(), base.to_string()))
                || renders(c, base, view.name.as_str())
        }) || named_anywhere.iter().any(|lit| names_template(lit, view.name.as_str()));
        if !reachable {
            unreachable.push(i);
        }
    }
    for i in unreachable {
        let view = &app.views[i];
        let mut d = crate::diagnostic::Diagnostic::unsupported(
            view.body.span,
            None,
            "template",
            format!(
                "`{}` is rendered by no action, route or render call, so Rails never reaches it; its errors are reported as warnings",
                view.name.as_str()
            ),
        );
        d.severity = crate::diagnostic::Severity::Warning;
        super::prism::push(d);
        app.unreachable_views.insert(view.name.clone());
    }
}

/// Whether the ancestry ends at Rails' own base: a gem's controller
/// (Doorkeeper's `AuthorizationsController`) brings actions not read here.
fn rooted_in_rails(controller: &Controller, by_name: &HashMap<&str, &Controller>) -> bool {
    let mut current = controller;
    for _ in 0..32 {
        let Some(parent) = current.parent.as_ref() else { return false };
        let name = parent.0.as_str().trim_start_matches("::");
        if matches!(name, "ActionController::Base" | "ActionController::API") {
            return true;
        }
        let Some(next) = by_name.get(name) else { return false };
        current = next;
    }
    false
}

/// The controller, its ancestors and every controller inheriting from it.
fn family<'a>(controller: &'a Controller, by_name: &HashMap<&str, &'a Controller>) -> Vec<&'a Controller> {
    let mut out = vec![controller];
    let mut current = controller;
    while let Some(parent) = current.parent.as_ref().and_then(|p| by_name.get(p.0.as_str())) {
        if out.iter().any(|c| c.name == parent.name) {
            break;
        }
        out.push(parent);
        current = parent;
    }
    let root = controller.name.0.as_str();
    for c in by_name.values() {
        let mut cur = *c;
        for _ in 0..32 {
            let Some(parent) = cur.parent.as_ref() else { break };
            if parent.0.as_str() == root {
                if !out.iter().any(|o| o.name == c.name) {
                    out.push(c);
                }
                break;
            }
            let Some(next) = by_name.get(parent.0.as_str()) else { break };
            cur = next;
        }
    }
    out
}

/// Whether `c` or a module it includes defines `name`; a module the app
/// does not define might, so it counts as defining it.
fn defines(c: &Controller, name: &str, modules: &HashMap<&str, HashSet<&str>>) -> bool {
    c.body.iter().any(|item| match item {
        ControllerBodyItem::Action { action, .. } => action.name.as_str() == name,
        ControllerBodyItem::Unknown { expr, .. } => included(expr).iter().any(|m| modules.get(m.as_str()).is_none_or(|defs| defs.contains(name))),
        _ => false,
    })
}

fn included(expr: &Expr) -> Vec<String> {
    let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return Vec::new() };
    if method.as_str() != "include" {
        return Vec::new();
    }
    args.iter()
        .filter_map(|a| match &*a.node {
            ExprNode::Const { path } => Some(path.iter().map(|s| s.as_str()).filter(|s| !s.is_empty()).collect::<Vec<_>>().join("::")),
            _ => None,
        })
        .collect()
}

/// Whether a render in `c` may reach the template: one naming it, or one
/// whose target is computed.
fn renders(c: &Controller, action: &str, template: &str) -> bool {
    let mut found = false;
    for item in &c.body {
        let body = match item {
            ControllerBodyItem::Action { action, .. } => &action.body,
            ControllerBodyItem::Unknown { expr, .. } => expr,
            _ => continue,
        };
        visit_renders(body, &mut |lits, dynamic| {
            found |= dynamic || lits.iter().any(|l| l == action || names_template(l, template));
        });
    }
    found
}

fn names_template(lit: &str, template: &str) -> bool {
    lit.trim_start_matches('/') == template
}

fn render_literals_outside_controllers(app: &App) -> Vec<String> {
    let mut out = Vec::new();
    let mut collect = |e: &Expr| visit_renders(e, &mut |lits, _| out.extend(lits.iter().cloned()));
    for lc in &app.library_classes {
        for m in &lc.methods {
            collect(&m.body);
        }
    }
    for v in &app.views {
        collect(&v.body);
    }
    out
}

/// Each render-like call under `e`: the String/Symbol literals among its
/// arguments, and whether its target is computed.
fn visit_renders(e: &Expr, f: &mut dyn FnMut(&[String], bool)) {
    e.node.for_each_child(&mut |c| visit_renders(c, f));
    let ExprNode::Send { method, args, .. } = &*e.node else { return };
    if !matches!(method.as_str(), "render" | "render_to_string" | "respond_with") {
        return;
    }
    let mut lits = Vec::new();
    let mut dynamic = method.as_str() == "respond_with";
    for (i, a) in args.iter().enumerate() {
        match &*a.node {
            ExprNode::Lit { value: Literal::Str { value } } => lits.push(value.clone()),
            ExprNode::Lit { value: Literal::Sym { value } } => lits.push(value.as_str().to_string()),
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    let key = match &*k.node {
                        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                        _ => continue,
                    };
                    if !matches!(key.as_str(), "action" | "template" | "file") {
                        continue;
                    }
                    match &*v.node {
                        ExprNode::Lit { value: Literal::Str { value } } => lits.push(value.clone()),
                        ExprNode::Lit { value: Literal::Sym { value } } => lits.push(value.as_str().to_string()),
                        _ => dynamic = true,
                    }
                }
            }
            _ if i == 0 => dynamic = true,
            _ => {}
        }
    }
    f(&lits, dynamic);
}
