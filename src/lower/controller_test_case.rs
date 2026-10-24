//! `ActionController::TestCase` (Rails 8.1, `ActionController::TestCase::
//! Behavior`): what a functional test needs that the test helper's
//! runtime class cannot work out by itself, because Spinel has no
//! `const_get` and no `instance_variables`.
//!
//! - The controller under test. Rails' `setup_controller_request_and_response`
//!   builds `@controller = <controller_class>.new` before the test's own
//!   setup, `controller_class` being the class the test names with
//!   `tests`, else the test class's name without `Test`
//!   (`ArticlesControllerTest` -> `ArticlesController`). An RSpec
//!   controller spec names it as the described class (the test module's
//!   `target`). Each test here starts with that assignment, and with the
//!   router symbol the runtime's route check needs (`:articles`).
//! - The instance variables a request leaves behind. Rails'
//!   `clear_instance_variables_between_requests` removes, before each
//!   request but the first, every instance variable the controller
//!   gained since the first one. A controller here gets a public
//!   `clear_instance_variables_between_requests` that sets to nil every
//!   instance variable its code (and its app ancestors' code) assigns;
//!   the runtime calls it before each request but the first.
//!
//! Runs at the end of ingest, before analysis, so what it writes is
//! typed like source. An app with no `ActionController::TestCase` test
//! is left unchanged.

use std::collections::HashMap;

use crate::app::App;
use crate::dialect::{Action, ControllerBodyItem, RenderTarget};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;

const TEST_CASE: &str = "ActionController::TestCase";
pub const CLEAR_METHOD: &str = "clear_instance_variables_between_requests";

pub fn prepare(app: &mut App) {
    if !app.test_modules.iter().any(|tm| tm.parent.as_ref().is_some_and(|p| p.0.as_str() == TEST_CASE)) {
        return;
    }
    add_clear_methods(app);
    let controllers: Vec<ClassId> = app.controllers.iter().map(|c| c.name.clone()).collect();
    for tm in &mut app.test_modules {
        if !tm.parent.as_ref().is_some_and(|p| p.0.as_str() == TEST_CASE) {
            continue;
        }
        let named = tm.name.0.as_str().strip_suffix("Test").map(|n| ClassId(Symbol::from(n)));
        let Some(controller) = tm
            .target
            .iter()
            .chain(named.iter())
            .find(|c| controllers.contains(c))
            .cloned()
        else {
            continue;
        };
        let span = Span::synthetic();
        let route = crate::lower::routes_to_library::controller_symbol(controller.0.as_str());
        let mut exprs = vec![
            assign(
                "controller",
                Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(Expr::new(span, ExprNode::Const { path: controller.0.as_str().split("::").map(Symbol::from).collect() })),
                        method: Symbol::from("new"),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                ),
            ),
            assign("__controller_route", Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(route.as_str()) } })),
        ];
        if let Some(setup) = tm.setup.take() {
            exprs.push(setup);
        }
        tm.setup = Some(Expr::new(span, ExprNode::Seq { exprs }));
    }
}

fn assign(ivar: &str, value: Expr) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Assign { target: LValue::Ivar { name: Symbol::from(ivar) }, value })
}

/// Every controller gets `clear_instance_variables_between_requests`,
/// public (ahead of any `private`), setting to nil the instance
/// variables its own code and its app ancestors' code assign.
fn add_clear_methods(app: &mut App) {
    let own: HashMap<ClassId, Vec<Symbol>> = app
        .controllers
        .iter()
        .map(|c| {
            let mut names = Vec::new();
            for item in &c.body {
                if let ControllerBodyItem::Action { action, .. } = item {
                    assigned_ivars(&action.body, &mut names);
                }
            }
            (c.name.clone(), names)
        })
        .collect();
    let parents: HashMap<ClassId, Option<ClassId>> = app.controllers.iter().map(|c| (c.name.clone(), c.parent.clone())).collect();
    for controller in &mut app.controllers {
        let mut chain = vec![controller.name.clone()];
        let mut at = controller.parent.clone();
        while let Some(p) = at {
            if chain.contains(&p) || !own.contains_key(&p) {
                break;
            }
            chain.push(p.clone());
            at = parents.get(&p).cloned().flatten();
        }
        let mut names: Vec<Symbol> = Vec::new();
        for c in chain.iter().rev() {
            for n in &own[c] {
                if !names.contains(n) {
                    names.push(n.clone());
                }
            }
        }
        let span = Span::synthetic();
        let nil = || Expr::new(span, ExprNode::Lit { value: Literal::Nil });
        let mut exprs: Vec<Expr> = names.iter().map(|n| assign(n.as_str(), nil())).collect();
        exprs.push(nil());
        let item = ControllerBodyItem::Action {
            action: Action {
                name: Symbol::from(CLEAR_METHOD),
                params: crate::ty::Row::default(),
                opt_params: Vec::new(),
                kw_params: Vec::new(),
                kwrest_param: None,
                block_param: None,
                rest_param: None,
                anonymous_formal: None,
                name_span: span,
                body: Expr::new(span, ExprNode::Seq { exprs }),
                renders: RenderTarget::Inferred,
                effects: crate::effect::EffectSet::pure(),
            },
            leading_comments: Vec::new(),
            leading_blank_line: true,
        };
        let at = controller
            .body
            .iter()
            .position(|item| matches!(item, ControllerBodyItem::PrivateMarker { .. }))
            .unwrap_or(controller.body.len());
        controller.body.insert(at, item);
    }
}

/// The instance variables `e` assigns (`@x = ...`, `@x ||= ...`), in
/// first-assignment order.
fn assigned_ivars(e: &Expr, out: &mut Vec<Symbol>) {
    let target = match &*e.node {
        ExprNode::Assign { target, .. } | ExprNode::OpAssign { target, .. } => Some(target),
        _ => None,
    };
    if let Some(LValue::Ivar { name }) = target {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    e.node.for_each_child(&mut |c| assigned_ivars(c, out));
}
