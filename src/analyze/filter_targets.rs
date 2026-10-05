//! A `before_action` / `around_action` / `after_action` whose target
//! method is defined nowhere the controller can reach.
//!
//! Rails registers the callback by name and looks the method up when an
//! action runs, so the class loads cleanly and every action the filter
//! applies to answers 500 (`NoMethodError`). Hifumi's generated Event
//! RSVP app shipped exactly that: `before_action :authenticate_user!` in
//! an app with no Devise, `GET /events/new` a 500. The lowering, finding
//! no body, drops the filter (`build_filter_preamble`), so the emitted
//! program does not fail where Rails does. It runs the action with no
//! guard in front of it. For an authentication filter that is an
//! unauthenticated write path, which is why this is an error and not a
//! warning.
//!
//! "Reachable" is the controller's own methods, its app ancestors', the
//! concern methods the ingest splice copied into either, and the
//! framework surface `registry::controllers` registers on
//! `ActionController::Base` (Devise's `authenticate_<scope>!` among it,
//! from the app's own `devise` declaration). The pass stays quiet where
//! the method could come from something it cannot see: a superclass that
//! is not an app controller or `ActionController::Base`/`API`, an
//! `include` of a module the app does not define, or a `define_method`.
//! An unknown gem that claims the name is attributed by
//! `attribution::attribute_unknown_gems`, like any dispatch failure.

use std::collections::{HashMap, HashSet};

use crate::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind, Severity};
use crate::dialect::{Controller, ControllerBodyItem, FilterKind};
use crate::expr::{ExprNode, Literal};
use crate::ident::{ClassId, Symbol};

/// Callbacks Rails defines on `ActionController::Base` that an app names
/// as filter targets but that are not part of the typed call surface.
const FRAMEWORK_CALLBACKS: &[&str] = &[
    crate::ingest::controller::VERIFY_AUTHENTICITY_TOKEN,
    "verify_same_origin_request",
    "mark_for_same_origin_verification!",
];

/// Roots an app controller chain may end in for the chain to be
/// complete: everything above them is the registered framework surface.
const FRAMEWORK_ROOTS: &[&str] = &["ActionController::Base", "ActionController::API"];

pub(super) fn diagnose(app: &App) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let framework = framework_methods(app);
    let modules: HashMap<&ClassId, &[ClassId]> =
        app.library_classes.iter().map(|lc| (&lc.name, lc.includes.as_slice())).collect();
    // An include the app defines is transparent only when everything it
    // includes is too: mastodon's `Authorization` concern includes
    // `Pundit::Authorization`, where `verify_authorized` lives.
    let opaque_module = |root: &ClassId| {
        let mut seen: HashSet<&ClassId> = HashSet::new();
        let mut stack = vec![root];
        while let Some(m) = stack.pop() {
            if !seen.insert(m) {
                continue;
            }
            let Some(nested) = modules.get(m) else { return true };
            stack.extend(nested.iter());
        }
        false
    };
    let opaque = |c: &Controller| {
        c.body.iter().any(defines_dynamically)
            || super::controller_include_groups(c).iter().flatten().any(|m| opaque_module(m))
    };
    for controller in &app.controllers {
        let Some(chain) = complete_chain(controller, &app.controllers, &opaque) else {
            continue;
        };
        // A filter declared on a base may name a method each subclass
        // defines; Rails only looks it up when an action runs, so those
        // count, and a subclass out of sight makes the absence unprovable.
        let descendants = descendants_of(controller, &app.controllers);
        if descendants.iter().any(|c| opaque(c)) {
            continue;
        }
        let defined: HashSet<&Symbol> = chain
            .iter()
            .chain(descendants.iter())
            .flat_map(|c| c.body.iter())
            .flat_map(defined_names)
            .collect();
        for item in &controller.body {
            let ControllerBodyItem::Filter { filter, .. } = item else { continue };
            if filter.kind.is_skip() || filter.target_span.is_synthetic() {
                continue;
            }
            let target = &filter.target;
            if defined.contains(target)
                || framework.contains(target)
                || FRAMEWORK_CALLBACKS.contains(&target.as_str())
            {
                continue;
            }
            let macro_name = match (&filter.kind, filter.prepend) {
                (FilterKind::Before, true) => "prepend_before_action",
                (FilterKind::Before, false) => "before_action",
                (FilterKind::Around, _) => "around_action",
                _ => "after_action",
            };
            let kind = DiagnosticKind::UndefinedFilterTarget {
                target: target.clone(),
                macro_name: Symbol::from(macro_name),
            };
            out.push(Diagnostic {
                span: filter.target_span,
                severity: Severity::Error,
                message: Diagnostic::stub_text(&kind),
                kind,
            });
        }
    }
    out
}

/// `controller` and its app ancestors, root-most last — or `None` when
/// some link of the chain is out of sight (a gem superclass, an
/// `include` the app does not define, `define_method`), so an absent
/// name proves nothing.
fn complete_chain<'a>(
    controller: &'a Controller,
    all: &'a [Controller],
    opaque: &dyn Fn(&Controller) -> bool,
) -> Option<Vec<&'a Controller>> {
    let mut chain = vec![controller];
    let mut cur = controller;
    loop {
        if opaque(cur) {
            return None;
        }
        let parent = cur.parent.as_ref()?;
        if FRAMEWORK_ROOTS.contains(&parent.0.as_str()) {
            return Some(chain);
        }
        let next = all.iter().find(|c| &c.name == parent)?;
        if chain.iter().any(|c| c.name == next.name) {
            return None;
        }
        chain.push(next);
        cur = next;
    }
}

/// Every app controller that inherits from `controller`, transitively.
fn descendants_of<'a>(controller: &Controller, all: &'a [Controller]) -> Vec<&'a Controller> {
    let mut out: Vec<&'a Controller> = Vec::new();
    let mut frontier = vec![controller.name.clone()];
    while let Some(name) = frontier.pop() {
        for c in all.iter().filter(|c| c.parent.as_ref() == Some(&name)) {
            if !out.iter().any(|o| o.name == c.name) {
                out.push(c);
                frontier.push(c.name.clone());
            }
        }
    }
    out
}

/// Method names a body item defines: an action/private method, or the
/// symbols of a defining macro (`attr_reader`, `delegate`, `alias_method`).
fn defined_names(item: &ControllerBodyItem) -> Vec<&Symbol> {
    match item {
        ControllerBodyItem::Action { action, .. } => vec![&action.name],
        ControllerBodyItem::Unknown { expr, .. } => {
            let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else {
                return vec![];
            };
            if !matches!(
                method.as_str(),
                "attr_reader" | "attr_accessor" | "attr_writer" | "delegate" | "alias_method"
            ) {
                return vec![];
            }
            args.iter()
                .filter_map(|a| match &*a.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => Some(value),
                    _ => None,
                })
                .collect()
        }
        _ => vec![],
    }
}

fn defines_dynamically(item: &ControllerBodyItem) -> bool {
    let ControllerBodyItem::Unknown { expr, .. } = item else { return false };
    matches!(
        &*expr.node,
        ExprNode::Send { recv: None, method, .. }
            if matches!(method.as_str(), "define_method" | "method_missing")
    )
}

/// The instance surface `registry::controllers` gives every controller,
/// run on its own: the same source the body typer resolves a bare call
/// in an action against, so a filter target and a call agree.
fn framework_methods(app: &App) -> HashSet<Symbol> {
    let mut classes = HashMap::new();
    super::registry::controllers::register(&mut classes, app, &[]);
    classes
        .get(&ClassId(Symbol::from("ActionController::Base")))
        .map(|acb| acb.class_methods.keys().chain(acb.instance_methods.keys()).cloned().collect())
        .unwrap_or_default()
}
