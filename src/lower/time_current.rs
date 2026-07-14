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
    super::for_each_hook_body(app, &mut rewrite_time_current);
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
        return;
    }
    // `t.httpdate` — stdlib-`time` sugar neither the CRuby tree
    // (without a `require "time"`) nor AOT targets know. Ground to
    // its definition: `t.getutc.strftime("%a, %d %b %Y %H:%M:%S
    // GMT")` — `getutc`, not `utc`, which mutates its receiver.
    // Shape-directed on the zero-arg name; `httpdate` is
    // Time-specific vocabulary.
    let is_httpdate = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if method.as_str() == "httpdate" && args.is_empty()
    );
    if is_httpdate {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv: Some(t), .. } = node else { unreachable!() };
        let getutc = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(t),
                method: Symbol::from("getutc"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let fmt = Expr::new(
            span,
            ExprNode::Lit {
                value: crate::expr::Literal::Str { value: "%a, %d %b %Y %H:%M:%S GMT".into() },
            },
        );
        *expr.node = ExprNode::Send {
            recv: Some(getutc),
            method: Symbol::from("strftime"),
            args: vec![fmt],
            block: None,
            parenthesized: true,
        };
    }
}
