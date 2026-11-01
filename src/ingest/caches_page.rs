//! actionpack-page_caching's `caches_page`, expanded to the
//! `after_action` the gem's macro registers (1.2.5,
//! `ActionController::Caching::Pages::ClassMethods#caches_page`):
//!
//! ```ruby
//! gzip_level = options.fetch(:gzip, page_cache_compression) # → nil, false, a Symbol or an Integer
//! after_action({ only: actions }.merge(options)) { |c| c.cache_page(nil, nil, gzip_level) }
//! ```
//!
//! so `caches_page :index, if: CACHE_PAGE` becomes a block-form
//! `after_action(only: [:index], if: <CACHE_PAGE's proc>) { cache_page(nil,
//! nil, nil) }`, which the filter chain already runs. The runtime half is
//! runtime/spinel/action_controller_page_caching.rb.
//!
//! A guard given as a constant (`if: CACHE_PAGE`, lobsters' spelling) is
//! the proc that constant holds, found on the controller or an ancestor:
//! Rails `instance_exec`s it on the controller, which is what a block
//! filter's guard body is. A shape this does not read (another option, a
//! guard constant it cannot find or that holds no proc, a
//! `page_cache_compression` set on a class) leaves the call as it was,
//! for `report_unrecognized_controller_macros` to report.

use crate::app::App;
use crate::dialect::{Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};

const IF_HOLE: &str = "RH_CACHES_PAGE_IF";
const UNLESS_HOLE: &str = "RH_CACHES_PAGE_UNLESS";

pub(super) fn expand(app: &mut App) {
    if app.controllers.iter().any(sets_page_cache_compression) {
        return;
    }
    for i in 0..app.controllers.len() {
        for j in 0..app.controllers[i].body.len() {
            let ControllerBodyItem::Unknown { expr, .. } = &app.controllers[i].body[j] else { continue };
            let Some(expanded) = expansion(app, &app.controllers[i], expr) else { continue };
            if let ControllerBodyItem::Unknown { expr, .. } = &mut app.controllers[i].body[j] {
                *expr = expanded;
            }
        }
    }
}

fn expansion(app: &App, controller: &Controller, expr: &Expr) -> Option<Expr> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else { return None };
    if method.as_str() != "caches_page" {
        return None;
    }
    let mut actions = Vec::new();
    let mut guards: Vec<(&str, Expr)> = Vec::new();
    let mut level = "nil".to_string();
    for arg in args {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => actions.push(value.clone()),
            ExprNode::Hash { entries, .. } => {
                for (k, v) in entries {
                    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
                    match key.as_str() {
                        "if" => guards.push(("if", guard(app, controller, v)?)),
                        "unless" => guards.push(("unless", guard(app, controller, v)?)),
                        "gzip" => level = gzip_level(v)?,
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
    if actions.is_empty() {
        return None;
    }
    let only = actions.iter().map(|a| format!(":{}", a.as_str())).collect::<Vec<_>>().join(", ");
    let mut options = format!("only: [{only}]");
    for (key, _) in &guards {
        options.push_str(&format!(", {key}: {}", if *key == "if" { IF_HOLE } else { UNLESS_HOLE }));
    }
    let src = format!("  after_action({options}) {{ cache_page(nil, nil, {level}) }}\n");
    let body = super::controller_macro_synth::reingest_controller_body(controller.name.0.as_str(), "<caches_page>", &src)?;
    let mut filter = body.into_iter().find_map(|item| match item {
        ControllerBodyItem::Unknown { expr, .. } => Some(expr),
        _ => None,
    })?;
    for (key, value) in guards {
        fill(&mut filter, if key == "if" { IF_HOLE } else { UNLESS_HOLE }, &value);
    }
    filter.span = expr.span;
    Some(filter)
}

/// The guard as a block filter takes it: a proc or lambda literal, a
/// Symbol, or the proc a constant on the controller's chain holds.
fn guard(app: &App, controller: &Controller, value: &Expr) -> Option<Expr> {
    match &*value.node {
        ExprNode::Lit { value: Literal::Sym { .. } } => Some(value.clone()),
        ExprNode::Const { path } => {
            let held = constant_value(app, controller, path)?;
            super::controller::ir_lambda_body(&held).is_some().then_some(held)
        }
        _ => super::controller::ir_lambda_body(value).is_some().then(|| value.clone()),
    }
}

fn constant_value(app: &App, controller: &Controller, path: &[Symbol]) -> Option<Expr> {
    let (name, owner) = match path {
        [name] => (name, None),
        [owner, name] => (name, Some(ClassId(owner.clone()))),
        _ => return None,
    };
    let mut current = match owner {
        Some(id) => app.controllers.iter().find(|c| c.name == id)?,
        None => controller,
    };
    loop {
        for item in &current.body {
            let ControllerBodyItem::Unknown { expr, .. } = item else { continue };
            if let ExprNode::Assign { target: LValue::Const { path }, value } = &*expr.node {
                if path.as_slice() == [name.clone()] {
                    return Some(value.clone());
                }
            }
        }
        let parent = current.parent.as_ref()?;
        current = app.controllers.iter().find(|c| &c.name == parent)?;
    }
}

/// The gem's `gzip_level` for an explicit `gzip:`.
fn gzip_level(value: &Expr) -> Option<String> {
    match &*value.node {
        ExprNode::Lit { value: Literal::Bool { value: false } } | ExprNode::Lit { value: Literal::Nil } => {
            Some("nil".to_string())
        }
        ExprNode::Lit { value: Literal::Int { value } } => Some(value.to_string()),
        ExprNode::Lit { value: Literal::Sym { value } } => Some(format!("Zlib::{}", value.as_str().to_uppercase())),
        ExprNode::Lit { value: Literal::Bool { value: true } } => Some("Zlib::BEST_COMPRESSION".to_string()),
        _ => None,
    }
}

fn sets_page_cache_compression(controller: &Controller) -> bool {
    fn visit(e: &Expr, found: &mut bool) {
        if let ExprNode::Send { method, .. } = &*e.node {
            if method.as_str() == "page_cache_compression=" {
                *found = true;
            }
        }
        e.node.for_each_child(&mut |c| visit(c, found));
    }
    let mut found = false;
    for item in &controller.body {
        if let ControllerBodyItem::Unknown { expr, .. } = item {
            visit(expr, &mut found);
        }
    }
    found
}

fn fill(e: &mut Expr, hole: &str, value: &Expr) {
    if matches!(&*e.node, ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == hole) {
        *e = value.clone();
        return;
    }
    e.node.for_each_child_mut(&mut |c| fill(c, hole, value));
}
