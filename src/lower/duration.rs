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
//! TIME INTEROP. A Duration only ever meets a `Time` through operators
//! Rails installs by reopening `Time`, which no transpiled runtime may
//! do. The CRuby overlay reopens it anyway (it is CRuby-only forever);
//! the spinel twin cannot, so every non-`.ago`/`.from_now` shape used
//! to reach a Duration object in an operator slot and die at runtime —
//! `Time - Duration` and `Time#after?` as `NoMethodError`, `Time <
//! Duration` as `comparison of Time with ActiveSupport::Duration
//! failed`. Three groundings below cover the shapes the corpus has,
//! each emitting what the overlay computes so the CRuby tree keeps
//! byte-parity while the AOT tree stops raising:
//!
//! - `after?` / `before?` → `>` / `<` (AS's readable comparators).
//! - `<t> - <dur>` / `+` → seconds via `.to_i`, the overlay's unwrap.
//! - `<time> <op> <bare dur>` → the ajd-vs-seconds arithmetic Rails
//!   reaches through `Time#<=>` → `to_datetime <=> other` →
//!   `Duration#coerce`. That comparison EVALUATES under Rails rather
//!   than raising (`Time.now <= 1.hour` is false, `<= 1.year` is true,
//!   verified against activesupport 8.1.3), and lobsters ships a
//!   dormant one — `story.rb`'s `send_referrer?` reads `created_at <=
//!   1.hour`, meaning `1.hour.ago`, on every story link. Grounding it
//!   to plain seconds (an earlier attempt) answers a DIFFERENT question
//!   and raises on CRuby, so the rewrite keeps the epoch offset:
//!   `t.to_f + 210_866_760_000.0 <op> dur.to_f * 86_400.0` — the
//!   overlay's `(to_r + 210_866_760_000r) / 86_400r <=> dur.seconds`
//!   with the division multiplied out, which keeps both sides one
//!   binop deep and needs no parenthesization from any emitter.
//!   Receiver-side only: a Duration RECEIVER compared against a Time
//!   raises under Rails too (`Duration#<=>` answers nil), so that
//!   shape stays untouched residue.
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
    let temporal_predicates = !app_defines_temporal_predicates(app);
    super::for_each_hook_body(app, &mut |body| {
        apply_duration_rewrites(body, temporal_predicates)
    });
}

/// The pass order the view vestige must mirror
/// (`emit::ruby::library::apply_duration_lowering`).
///
/// `rewrite_durations` first: everything downstream matches on the
/// grounded `ActiveSupport::Duration.<unit>(n)` shape. Predicates next,
/// so a rewritten `after?` reaches the comparison rules as the `>` it
/// means. Comparisons BEFORE arithmetic: both read a bare Duration
/// operand, and the arithmetic rewrite hides one behind `.to_i` — after
/// it runs, `<t> - <dur>` is no longer distinguishable from the numeric
/// `Time - Time` shape.
pub(crate) fn apply_duration_rewrites(body: &mut Expr, temporal_predicates: bool) {
    rewrite_durations(body);
    if temporal_predicates {
        rewrite_temporal_predicates(body);
    }
    rewrite_duration_comparisons(body);
    rewrite_time_duration_arith(body);
}

/// Does any app class define its own `after?` / `before?`? Then the
/// name doesn't mean ActiveSupport's temporal comparator and the
/// predicate rewrite stands down wholesale — the same coarse opt-out
/// `exclude_predicate` takes, and for the same reason: the receiver is
/// rarely `self`, so a per-class check would not see the collision.
pub(crate) fn app_defines_temporal_predicates(app: &App) -> bool {
    let is_pred = |n: &Symbol| matches!(n.as_str(), "after?" | "before?");
    app.models.iter().any(|m| {
        m.body.iter().any(|item| {
            matches!(item, crate::dialect::ModelBodyItem::Method { method, .. }
                if is_pred(&method.name))
        })
    }) || app
        .library_classes
        .iter()
        .any(|c| c.methods.iter().any(|m| is_pred(&m.name)))
}

/// `<t>.after?(x)` → `<t> > x`, `<t>.before?(x)` → `<t> < x`.
///
/// AS defines these on Date/Time/DateTime as literally `self > other` /
/// `self < other`, so the rewrite is total and needs no receiver type —
/// which is what lets it ground the untyped view receivers the
/// commentbox partial hands it (`(story.created_at - 1.hour).before?`).
fn rewrite_temporal_predicates(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_temporal_predicates);
    let op = match &*expr.node {
        ExprNode::Send { recv: Some(_), method, args, block: None, .. } if args.len() == 1 => {
            match method.as_str() {
                "after?" => ">",
                "before?" => "<",
                _ => return,
            }
        }
        _ => return,
    };
    let ExprNode::Send { method, .. } = &mut *expr.node else { unreachable!() };
    *method = Symbol::from(op);
    expr.ty = Some(Ty::Bool);
}

/// `<t> - <dur>` / `<t> + <dur>` → the Duration's seconds, which is
/// what the CRuby overlay's `Time#-` reopen unwraps to before calling
/// through. `Time ± Integer` is a `Time` on every ruby-family runtime
/// (spinel included), so the outer node keeps its own type and span.
///
/// A Duration RECEIVER is left alone: `1.day - 1.hour` is Duration
/// arithmetic, and the value class ships no operators, so grounding
/// only one operand would quietly change what the expression means.
fn rewrite_time_duration_arith(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite_time_duration_arith);
    let is_arith = matches!(
        &*expr.node,
        ExprNode::Send { recv: Some(recv), method, args, block: None, .. }
            if args.len() == 1
                && matches!(method.as_str(), "-" | "+")
                && is_duration_const_call(&args[0])
                && !is_duration_const_call(recv)
    );
    if !is_arith {
        return;
    }
    let ExprNode::Send { args, .. } = &mut *expr.node else { unreachable!() };
    let dur = take(&mut args[0]);
    let span = dur.span;
    args[0] = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(dur),
            method: Symbol::from("to_i"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    args[0].ty = Some(Ty::Int);
}

/// Move `side` out, leaving the empty-`Seq` placeholder this file uses
/// for in-place operand surgery.
fn take(side: &mut Expr) -> Expr {
    let span = side.span;
    std::mem::replace(side, Expr::new(span, ExprNode::Seq { exprs: vec![] }))
}

/// `<e>.to_f`, stamped `Float`.
fn to_f(inner: Expr) -> Expr {
    let span = inner.span;
    let mut e = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(inner),
            method: Symbol::from("to_f"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    e.ty = Some(Ty::Float);
    e
}

/// `<lhs> <op> <n>`, stamped `Float`. Both call sites bind tighter than
/// the comparison they land inside, so neither needs parentheses.
fn float_binop(lhs: Expr, op: &str, rhs: f64) -> Expr {
    let span = lhs.span;
    let mut lit = Expr::new(span, ExprNode::Lit { value: Literal::Float { value: rhs } });
    lit.ty = Some(Ty::Float);
    let mut e = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(lhs),
            method: Symbol::from(op),
            args: vec![lit],
            block: None,
            parenthesized: false,
        },
    );
    e.ty = Some(Ty::Float);
    e
}

/// Seconds from the astronomical Julian day epoch to the Unix epoch —
/// `2_440_587.5 * 86_400`, the constant Rails' `Time#to_datetime`
/// conversion puts in front of `Duration#coerce`.
const AJD_EPOCH_SECONDS: f64 = 210_866_760_000.0;
const SECONDS_PER_DAY: f64 = 86_400.0;

/// A Duration compared against a NUMERIC scalar grounds to its
/// seconds: `Time.now.utc - created_at <= 70.days` — the left side is
/// Float seconds after the temporal lowering, and nothing coerces the
/// Duration operand on strict targets (Rails resolves it through
/// Duration#coerce, comparing seconds — `.to_f` is identical).
///
/// A TIME compared against a bare Duration takes the second rule
/// below instead: Rails EVALUATES that shape rather than raising, so
/// grounding it to plain seconds answers a different question — and
/// raised on every /newest render when an earlier attempt did exactly
/// that. See the module header's TIME INTEROP note.
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

    // Rule 1 — numeric vs Duration grounds to seconds.
    if is_numeric_side(recv) || is_numeric_side(&args[0]) {
        for side in [recv as &mut Expr].into_iter().chain(args.iter_mut()) {
            if is_duration_const_call(side) {
                let grounded = to_f(take(side));
                *side = grounded;
            }
        }
        return;
    }

    // Rule 2 — a Time RECEIVER vs a bare Duration takes Rails'
    // ajd-vs-seconds coercion, division multiplied out.
    if recv.ty.as_ref().is_some_and(Ty::contains_time) && is_duration_const_call(&args[0]) {
        let t = take(recv);
        *recv = float_binop(to_f(t), "+", AJD_EPOCH_SECONDS);
        let dur = take(&mut args[0]);
        args[0] = float_binop(to_f(dur), "*", SECONDS_PER_DAY);
    }
}

/// Is this comparison operand the NUMERIC side — a stamped Int/Float,
/// or a subtraction yielding Float seconds (`Time.now.utc -
/// created_at`)?
///
/// A subtraction whose operand is a Duration is excluded: `Time -
/// Duration` is another Time, and treating it as numeric would ground
/// the Duration on the far side to bare seconds and raise on CRuby.
fn is_numeric_side(s: &Expr) -> bool {
    if is_duration_const_call(s) {
        return false;
    }
    if matches!(s.ty, Some(Ty::Float) | Some(Ty::Int)) {
        return true;
    }
    matches!(&*s.node,
        ExprNode::Send { method, args, block: None, .. }
            if method.as_str() == "-" && args.len() == 1 && !is_duration_const_call(&args[0]))
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
