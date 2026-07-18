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
//! `tests/send_dispatch_lowering.rs`). This constraint is declared
//! canonically in `lower::POST_ANALYZE_PASS_ORDER` (the `duration`
//! entry's `runs_after`).
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
    super::for_each_hook_body(app, &mut rewrite_duration_comparisons);
}

/// A Duration compared against a NUMERIC scalar grounds to its
/// seconds: `Time.now.utc - created_at <= 70.days` — the left side is
/// Float seconds after the temporal lowering, and nothing coerces the
/// Duration operand on strict targets (Rails resolves it through
/// Duration#coerce, comparing seconds — `.to_f` is identical).
///
/// STRICTLY numeric-vs-Duration: a TIME compared against a bare
/// Duration must stay untouched — Rails EVALUATES that shape
/// (compare_with_coercion's ajd-vs-seconds arithmetic; lobsters ships
/// the dormant `created_at <= 1.hour` and Rails answers false), and
/// the CRuby overlay's Time reopen mirrors it by intercepting the
/// Duration operand. Rewriting it to Float bypassed the intercept and
/// raised on every /newest render.
fn rewrite_duration_comparisons(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_duration_comparisons);
    let is_cmp = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(_), method, args, block: None, .. }
            if args.len() == 1
                && matches!(method.as_str(), "<" | "<=" | ">" | ">=")
    );
    if !is_cmp {
        return;
    }
    let ExprNode::Send { recv: Some(recv), args, .. } = &mut *expr.node else { unreachable!() };
    let mut sides: Vec<&mut Expr> = [recv as &mut Expr].into_iter().chain(args.iter_mut()).collect();
    // The numeric side is a SUBTRACTION (`Time.now.utc - created_at`,
    // Float seconds) or a stamped numeric; a bare temporal read stays
    // un-numeric and blocks the rewrite.
    let numeric_other = sides.iter().any(|s| {
        !is_duration_const_call(s)
            && (matches!(s.ty, Some(Ty::Float) | Some(Ty::Int))
                || matches!(&*s.node,
                    ExprNode::Send { method, args, block: None, .. }
                        if method.as_str() == "-" && args.len() == 1))
    });
    if !numeric_other {
        return;
    }
    for side in sides.iter_mut() {
        if is_duration_const_call(side) {
            let span = side.span;
            let inner = std::mem::replace(
                &mut **side,
                Expr::new(span, ExprNode::Seq { exprs: vec![] }),
            );
            **side = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(inner),
                    method: Symbol::from("to_f"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            );
            side.ty = Some(Ty::Float);
        }
    }
}

/// `ActiveSupport::Duration.<unit>(n)` — the shape `rewrite_durations`
/// itself produces.
fn is_duration_const_call(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: Some(r), method, block: None, .. }
            if is_duration_unit(method.as_str())
                && matches!(&*r.node,
                    ExprNode::Const { path } if path.len() == 2
                        && path[0].as_str() == "ActiveSupport"
                        && path[1].as_str() == "Duration")
    )
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
