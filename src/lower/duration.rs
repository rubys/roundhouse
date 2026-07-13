//! ActiveSupport duration builders → `ActiveSupport::Duration` class
//! calls: `70.days` / `NEW_USER_DAYS.days` / `1.week` would dispatch a
//! nonexistent `Integer#days` on every tree — reopening `Integer` in
//! the shared runtime is off-limits (no built-in reopening; `Time`
//! arithmetic doesn't transpile uniformly) — so the builder grounds to
//! `ActiveSupport::Duration.days(70)`, the value class the ruby-family
//! trees ship (CRuby overlay `active_support_duration.rb`, spinel twin
//! minus the `Time` reopen). `.ago` / `.from_now` then ride the
//! returned instance and need no rewrite. Strict targets carry the
//! grounded call as honest residue — ONE named runtime seam (provide a
//! Duration value class) instead of scattered nonexistent-Int-method
//! sends; widening them is a separate step (the Time-arith seam), same
//! posture as `send_dispatch`'s plural arms before this pass existed.
//!
//! ORDER: must run AFTER `apply_send_static_dispatch` in the
//! post-analyze hook — for an all-duration-unit name set, dispatch
//! synthesizes its case arms as plural unit calls counting on this
//! pass to ground them (`send_dispatch::duration_plural`; guard test
//! `tests/send_dispatch_lowering.rs`).
//!
//! Collision gate: the singular `day`/`hour`/`month`/`year` also name
//! `Time` component readers (`created_at.day`, lobsters
//! traffic_helper's `time.month == 6`), so those rewrite only when the
//! receiver is provably numeric — an Int/Float literal or a
//! typer-stamped Int/Float. Every plural, plus `minute`/`second`/
//! `week`/`fortnight`, never collides and rewrites unconditionally, so
//! an Int constant receiver whose type stays unresolved
//! (`NEW_USER_DAYS.days`) still lands. No residue policy: a skipped
//! colliding singular is a `Time` component read in every corpus
//! occurrence, and ledgering each legitimate `time.day` would be
//! noise — the emit-time ancestor of this pass was silent for the
//! same reason.
//!
//! View bodies are deliberately not walked — the same carve-out as
//! blank/time_current: the ruby view pipeline still applies the
//! emit-time vestige (`emit::ruby::library::apply_duration_lowering`)
//! to lowered view classes (lobsters' `_commentbox.html.erb` compares
//! against `COMMENTABLE_DAYS.days.ago`); views rejoin when the view
//! pipeline migrates to shared lowerings.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// Ground duration-unit sends across every app body the post-analyze
/// hook owns (models, library classes, controllers, seeds — not views).
pub fn apply_duration_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite_durations);
}

/// ActiveSupport duration unit method names (`70.days`, `1.week`). The
/// singular `day`/`hour`/`month`/`year` also name `Time` component
/// readers (`created_at.day`), so those rewrite only when the receiver
/// is numeric; the others never collide and rewrite unconditionally.
fn duration_unit_collides_with_time(unit: &str) -> bool {
    matches!(unit, "day" | "hour" | "month" | "year")
}

fn is_duration_unit(unit: &str) -> bool {
    matches!(
        unit,
        "days" | "day" | "hours" | "hour" | "minutes" | "minute" | "seconds" | "second"
            | "weeks" | "week" | "fortnights" | "fortnight" | "months" | "month" | "years" | "year"
    )
}

/// Is `e` a numeric value — an Int/Float literal or an expression the
/// typer resolved to `Int`/`Float`? (Keeps `created_at.day` — a
/// datetime — out of the colliding-unit rewrite.)
fn is_numeric_expr(e: &Expr) -> bool {
    if matches!(&*e.node, ExprNode::Lit { value: Literal::Int { .. } })
        || matches!(&*e.node, ExprNode::Lit { value: Literal::Float { .. } })
    {
        return true;
    }
    matches!(&e.ty, Some(Ty::Int) | Some(Ty::Float))
}

/// `<n>.days` → `ActiveSupport::Duration.days(<n>)`, in place,
/// recursively. The receiver moves into argument position keeping its
/// stamped type. The synthesized `ActiveSupport::Duration` const is
/// stamped `Ty::Class` — what analyze stamps for any multi-segment
/// const — because the residual-diagnostics audit walks hook output
/// and an unstamped const reads as an unresolved name. The outer send
/// keeps the site's own type (`Int#days` types as the gradual escape,
/// so the stamp usually stays `Untyped`); a site the typer left open
/// takes `Untyped` too — the honest type of a call whose runtime class
/// the registry doesn't model (`send_dispatch`'s fallback convention).
/// Also the implementation behind the ruby emitter's view-pipeline
/// vestige.
pub(crate) fn rewrite_durations(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_durations);
    let rewrite = match &*expr.node {
        ExprNode::Send { recv: Some(r), method, args, block: None, .. }
            if args.is_empty() && is_duration_unit(method.as_str()) =>
        {
            !duration_unit_collides_with_time(method.as_str()) || is_numeric_expr(r)
        }
        _ => false,
    };
    if rewrite {
        let span = expr.span;
        let node = std::mem::replace(&mut *expr.node, ExprNode::Seq { exprs: vec![] });
        let ExprNode::Send { recv, method, .. } = node else { unreachable!() };
        let arg = recv.expect("duration send has a receiver");
        let path = vec![Symbol::from("ActiveSupport"), Symbol::from("Duration")];
        let mut duration_const = Expr::new(span, ExprNode::Const { path });
        duration_const.ty = Some(Ty::Class {
            id: ClassId(Symbol::from("ActiveSupport::Duration")),
            args: vec![],
        });
        *expr.node = ExprNode::Send {
            recv: Some(duration_const),
            method,
            args: vec![arg],
            block: None,
            parenthesized: true,
        };
        if matches!(expr.ty, None | Some(Ty::Var { .. })) {
            expr.ty = Some(Ty::Untyped);
        }
    }
}
