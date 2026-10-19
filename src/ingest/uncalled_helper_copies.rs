//! A controller that `include`s an `app/helpers` module got a copy of
//! each of its methods (`splice_concerns_into_controllers`), and the copy
//! is typed as controller code. Views do not use it — they call the
//! helper module itself — so in a copy no controller calls, a `t` is the
//! view's, never the controller's: marked here
//! ([`crate::expr::HELPER_COPY_TRANSLATE`]) so it takes ActionView's
//! translate, as the helper module's own body does. mastodon's
//! `RegistrationHelper#terms_agreement_label` is the case: three
//! controllers include it for `allowed_registration?`, none calls the
//! label, and its `_html` translations were refused as controller code.
//!
//! Not dropped: the copy is the helper's body as controller code, which
//! is what a controller that does call it runs.
//!
//! A copy counts as called when a controller of its family (the
//! includer, its ancestors, which run with a subclass as `self`, and
//! its descendants, which inherit the copy) names it: a call, or a
//! String / Symbol literal (`before_action :x`, `helper_method :x`,
//! `send(:x)`), or a route's action.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::ControllerBodyItem;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

pub(super) fn mark(app: &mut App) {
    let helpers: HashSet<_> = app.helper_method_index.values().cloned().collect();
    // After the splice, so the controllers' copies keep their own reading.
    let modules: HashSet<_> = helpers.iter().chain(app.helper_modules.iter()).cloned().collect();
    for lc in app.library_classes.iter_mut().filter(|lc| modules.contains(&lc.name)) {
        for m in &mut lc.methods {
            for default in m.params.iter_mut().filter_map(|p| p.default.as_mut()) {
                mark_with(default, crate::expr::HELPER_BODY_TRANSLATE);
            }
            mark_with(&mut m.body, crate::expr::HELPER_BODY_TRANSLATE);
        }
    }
    if helpers.is_empty() || app.concern_spliced_actions.is_empty() {
        return;
    }
    let mut named = names_in_controllers(app);
    for r in crate::lower::routes::flatten_routes_for_helpers(app) {
        named.entry(r.controller.clone()).or_default().insert(r.action.as_str().to_string());
    }
    let mut uncalled: Vec<(crate::ident::ClassId, Symbol)> = Vec::new();
    for (controller, spliced) in &app.concern_spliced_actions {
        let family = family(app, controller);
        for (method, module) in spliced {
            let name = method.as_str();
            if helpers.contains(module) && !family.iter().any(|c| named.get(c).is_some_and(|n| n.contains(name))) {
                uncalled.push((controller.clone(), method.clone()));
            }
        }
    }
    for (controller, method) in uncalled {
        let Some(c) = app.controllers.iter_mut().find(|c| c.name == controller) else { continue };
        for item in &mut c.body {
            if let ControllerBodyItem::Action { action, .. } = item
                && action.name == method
            {
                for (_, default) in &mut action.opt_params {
                    mark_translate(default);
                }
                for default in action.kw_params.iter_mut().filter_map(|(_, d)| d.as_mut()) {
                    mark_translate(default);
                }
                mark_translate(&mut action.body);
            }
        }
    }
}

fn mark_translate(e: &mut Expr) {
    mark_with(e, crate::expr::HELPER_COPY_TRANSLATE);
}

fn mark_with(e: &mut Expr, bit: u64) {
    e.node.for_each_child_mut(&mut |c| mark_with(c, bit));
    if let ExprNode::Send { recv: None, method, .. } = &*e.node
        && matches!(method.as_str(), "t" | "translate")
    {
        e.decisions |= bit;
    }
}

/// The controller, its ancestors and its descendants among the app's controllers.
fn family(app: &App, controller: &ClassId) -> Vec<ClassId> {
    let parent_of = |c: &ClassId| app.controllers.iter().find(|x| x.name == *c).and_then(|x| x.parent.clone());
    let mut out = vec![controller.clone()];
    let mut cur = parent_of(controller);
    while let Some(p) = cur {
        if out.contains(&p) {
            break;
        }
        cur = parent_of(&p);
        out.push(p);
    }
    for c in &app.controllers {
        let mut cur = c.parent.clone();
        for _ in 0..32 {
            let Some(p) = cur else { break };
            if p == *controller {
                if !out.contains(&c.name) {
                    out.push(c.name.clone());
                }
                break;
            }
            cur = parent_of(&p);
        }
    }
    out
}

/// Every method name each controller body calls or spells as a literal.
fn names_in_controllers(app: &App) -> HashMap<ClassId, HashSet<String>> {
    let mut by_controller = HashMap::new();
    for c in &app.controllers {
        let out: &mut HashSet<String> = by_controller.entry(c.name.clone()).or_default();
        for item in &c.body {
            let exprs: Vec<&Expr> = match item {
                ControllerBodyItem::Action { action, .. } => {
                    let mut v = vec![&action.body];
                    v.extend(action.opt_params.iter().map(|(_, d)| d));
                    v.extend(action.kw_params.iter().filter_map(|(_, d)| d.as_ref()));
                    v
                }
                ControllerBodyItem::ClassMethod { method, .. } => vec![&method.body],
                ControllerBodyItem::ClassIvarInit { expr, .. } | ControllerBodyItem::Unknown { expr, .. } => vec![expr],
                ControllerBodyItem::Filter { filter, .. } => {
                    out.insert(filter.target.as_str().to_string());
                    out.extend(filter.if_cond.iter().chain(filter.unless_cond.iter()).map(|s| s.as_str().to_string()));
                    filter.if_cond_expr.iter().chain(filter.unless_cond_expr.iter()).chain(filter.block.iter()).collect()
                }
                _ => Vec::new(),
            };
            for e in exprs {
                collect(e, out);
            }
        }
    }
    by_controller
}

fn collect(e: &Expr, out: &mut HashSet<String>) {
    e.node.for_each_child(&mut |c| collect(c, out));
    match &*e.node {
        ExprNode::Send { method, .. } => {
            out.insert(method.as_str().to_string());
        }
        ExprNode::Lit { value: Literal::Sym { value } } => {
            out.insert(value.as_str().to_string());
        }
        ExprNode::Lit { value: Literal::Str { value } } => {
            out.insert(value.clone());
        }
        _ => {}
    }
}
