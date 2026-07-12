//! `Time.current` grounding: Rails' zone-aware now → `Time.now.utc`.
//!
//! Plain Ruby has no `Time.current` — the Rails-ism is as undefined on
//! the CRuby tree as under spinel AOT, just lazily so — and the corpus
//! apps run UTC, where the two are second-for-second equivalent.
//! Grounding it here keeps `Time` un-reopened (built-in reopening in
//! the shared runtime is off-limits) and lands on vocabulary every
//! emitter already speaks: all target families handle `Time.now` and a
//! zero-arg `.utc`, while only the ruby family ever knew `current`.
//!
//! Runs post-analyze (with `apply_blank_lowering`, see
//! `apply_post_analyze_lowerings`): the rewrite is shape-directed, not
//! type-directed, but rewriting after the analyzer means `Time.current`
//! stays typeable as a registered class method and the new nodes can be
//! stamped from the types analyze already assigned. No residue policy —
//! the match (`Time.current`, zero args, no block) is unconditional and
//! the rewrite total, so there is no diagnostic to return.
//!
//! View bodies are deliberately not walked — same carve-out as the
//! blank pass: the view pipeline still applies the ruby-family emit
//! copy of this rewrite (`emit::ruby::library::apply_time_current_
//! lowering`) to lowered view classes, and rejoins the shared home when
//! views migrate. Test-module and fixture bodies are not walked either
//! (they run on CRuby; extendable when a strict-target test lane needs
//! it).

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

/// Rewrite `Time.current` sends across every app body the post-analyze
/// hook owns (models, library classes, controllers, seeds — not views).
pub fn apply_time_current_lowering(app: &mut App) {
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => {
                    rewrite_time_current(&mut method.body);
                }
                crate::dialect::ModelBodyItem::Scope { scope, .. } => {
                    rewrite_time_current(&mut scope.body);
                }
                crate::dialect::ModelBodyItem::Callback { callback, .. } => {
                    if let Some(cond) = &mut callback.condition {
                        rewrite_time_current(cond);
                    }
                }
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => {
                    rewrite_time_current(expr);
                }
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        for method in &mut lc.methods {
            rewrite_time_current(&mut method.body);
        }
    }
    for controller in &mut app.controllers {
        for item in &mut controller.body {
            match item {
                crate::dialect::ControllerBodyItem::Action { action, .. } => {
                    rewrite_time_current(&mut action.body);
                }
                crate::dialect::ControllerBodyItem::Unknown { expr, .. } => {
                    rewrite_time_current(expr);
                }
                _ => {}
            }
        }
    }
    if let Some(seeds) = &mut app.seeds {
        rewrite_time_current(seeds);
    }
}

/// `Time.current` → `Time.now.utc`, in place, recursively. The original
/// `Time` const node moves into the new tree (keeping its stamped type),
/// the synthesized `now` send takes the site's own type (`Time.now` and
/// `Time.current` type identically), and the outer expr keeps its type.
/// Also the implementation behind the ruby emitter's view-pipeline copy.
pub(crate) fn rewrite_time_current(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_time_current);
    let is_target = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if method.as_str() == "current"
                && args.is_empty()
                && matches!(&*r.node,
                    ExprNode::Const { path } if path.len() == 1 && path[0].as_str() == "Time")
    );
    if is_target {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv: Some(time_const), .. } = node else { unreachable!() };
        let mut now = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(time_const),
                method: Symbol::from("now"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        now.ty = expr.ty.clone();
        *expr.node = ExprNode::Send {
            recv: Some(now),
            method: Symbol::from("utc"),
            args: vec![],
            block: None,
            parenthesized: false,
        };
    }
}
