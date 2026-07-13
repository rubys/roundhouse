//! ActiveSupport blank-predicate lowering: ground `blank?` /
//! `present?` / `presence` by the receiver's *static* type so every
//! target compiles the site without an `Object` monkey-patch.
//!
//! ActiveSupport ships these as core_ext reopens of `Object`/`String`
//! (`respond_to?(:empty?) ? !!empty? : !self`) — a shape only the
//! CRuby overlay can host. Every other target either can't reopen
//! builtins (transpiled runtimes) or can't dispatch a user-defined
//! method on an untyped value at all (spinel AOT). The types the
//! analyzer already stamped make the dynamic dispatch unnecessary at
//! almost every site, so this pass rewrites, per receiver type:
//!
//!   String        blank? → `r.empty?` — deliberately NOT the
//!                 whitespace-aware AS form (`" ".blank?` is true in
//!                 Rails, false here). Two reasons: it matches the
//!                 form the view-cond predicate rewrite
//!                 (`view_to_library::predicates`) has always
//!                 produced, and the ruby-family nil-safety patch
//!                 (`apply_nilsafe_empty_lowering`) guards the direct
//!                 receiver of `empty?` — an interposed `.strip`
//!                 would put the guard on the wrong level for
//!                 nullable columns the type layer reads as `Str`.
//!                 Upgrade to whitespace-aware together with honest
//!                 column nilability.
//!   Array/Hash    blank? → `r.empty?`
//!   Class w/ own  left as-is when the app class defines the
//!   definition    predicate itself; grounded via `empty?` when it
//!                 defines that (`ActiveRecord::Relation` included)
//!   other Class / blank? → `false` (a non-nil value with no `empty?`
//!   Int/Float/    is never blank — matches AS, where `0` and
//!   Sym/Time      `Time.now` are present)
//!   Bool          blank? → `!r` (`false` is blank)
//!   T | Nil       nil-check composed with the T grounding
//!
//! `present?` is the negation; `presence` is `blank? ? nil : r`
//! (an `If` node), or just `r` where T is never blank.
//!
//! Residue policy: a receiver the pass can't ground — `untyped`, an
//! open inference var, a multi-variant union — keeps its dynamic call
//! and gets a `blank_unlowered` warning naming the site. That list is
//! the ledger: on CRuby the overlay still serves the call at runtime;
//! on AOT/strict targets each entry is a named per-target gap instead
//! of a silent compile error. Same policy when a form that must
//! re-evaluate (or drop) the receiver meets a receiver with effects:
//! skip and report rather than change evaluation order.
//!
//! Test-module and fixture bodies are not walked (they run on CRuby
//! where the overlay serves the dynamic call); extendable when a
//! strict-target test lane needs it. View bodies are not walked
//! either — see the note in [`apply_blank_lowering`].

use crate::app::App;
use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::expr::{
    Arm, Expr, ExprNode, InterpPart, LValue, Literal, RescueClause,
};
use crate::ident::Symbol;
use crate::ty::Ty;
use std::collections::HashSet;

/// Rewrite blank-predicate sends across every typed body in the app.
/// Runs after `Analyzer::analyze` (receiver types must be stamped) and
/// before any emitter. Returns the residue diagnostics — sites left as
/// dynamic dispatch, with the reason.
pub fn apply_blank_lowering(app: &mut App) -> Vec<Diagnostic> {
    let defs = AppDefinitions::collect(app);
    let mut diags = Vec::new();

    // View bodies are deliberately NOT walked (`for_each_hook_body`
    // excludes them). Every target already has working view-cond
    // predicate handling — the shared `view_to_library::predicates`
    // rewrite for the ruby/spinel family, and the python/rust view
    // emitters' own vocabulary — and those walkers match the ORIGINAL
    // `present?`/`blank?` shapes. Rewriting under them breaks the ones
    // with closed vocabularies (python's unemittable-cond fallback is a
    // silent `False`; the flash-notice smoke caught it). Views rejoin
    // when the view pipeline migrates to shared lowerings.
    super::for_each_hook_body(app, &mut |body| walk(body, &defs, &mut diags));

    diags
}

/// Which classes define their own `blank?`/`present?`/`presence`
/// (leave the dispatch alone) or an `empty?` (ground through it).
/// Keyed by the class name's last segment — the same resolution
/// `Ty::Class` receivers get elsewhere.
struct AppDefinitions {
    own_predicate: HashSet<String>,
    own_empty: HashSet<String>,
}

impl AppDefinitions {
    fn collect(app: &App) -> Self {
        let mut own_predicate = HashSet::new();
        let mut own_empty = HashSet::new();
        let mut note = |class: &str, method: &str| {
            let last = class.rsplit("::").next().unwrap_or(class).to_string();
            match method {
                "blank?" | "present?" | "presence" => {
                    own_predicate.insert(last);
                }
                "empty?" => {
                    own_empty.insert(last);
                }
                _ => {}
            }
        };
        for model in &app.models {
            for method in model.methods() {
                note(model.name.0.as_str(), method.name.as_str());
            }
        }
        for lc in &app.library_classes {
            for method in &lc.methods {
                note(lc.name.0.as_str(), method.name.as_str());
            }
        }
        Self { own_predicate, own_empty }
    }
}

/// How a receiver type grounds the predicate.
enum Grounding {
    /// `empty?` applies (strings and collections).
    Container { nilable: bool },
    /// Never blank when non-nil (numbers, symbols, times, plain
    /// objects without `empty?`).
    NeverBlank { nilable: bool },
    /// Truthiness is the answer (`false` is blank).
    BoolLike,
    /// The receiver is statically nil.
    AlwaysNil,
    /// The class defines the predicate itself — normal dispatch.
    OwnDispatch,
    /// Can't ground; leave the call and report.
    Skip(&'static str),
}

fn classify(ty: Option<&Ty>, defs: &AppDefinitions) -> Grounding {
    use Grounding::*;
    let Some(t) = ty else { return Skip("receiver type not inferred") };
    match t {
        Ty::Str | Ty::Array { .. } | Ty::Hash { .. } | Ty::Tuple { .. } => {
            Container { nilable: false }
        }
        Ty::Int | Ty::Float | Ty::Sym | Ty::Time | Ty::Record { .. } => {
            NeverBlank { nilable: false }
        }
        Ty::Bool => BoolLike,
        Ty::Nil => AlwaysNil,
        Ty::Class { id, .. } => {
            let raw = id.0.as_str();
            let last = raw.rsplit("::").next().unwrap_or(raw);
            if defs.own_predicate.contains(last) {
                OwnDispatch
            } else if last == "Relation" || last == "Errors" || defs.own_empty.contains(last) {
                // Registry classes the analyzer types but the app
                // doesn't define: ActiveRecord::Relation and
                // ActiveModel::Errors both answer `empty?` (the
                // transpiled runtime's `errors` reader is an Array).
                // Folding either to never-blank would render
                // errors_for-style guards unconditionally.
                Container { nilable: false }
            } else if last == "ParamValue" {
                // Param access wraps possibly-absent input; blankness
                // is semantic there and needs a runtime predicate on
                // the ParamValue type, not a fold.
                Skip("ParamValue receiver needs a runtime predicate")
            } else {
                NeverBlank { nilable: false }
            }
        }
        Ty::Union { variants } => {
            let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
            let non_nil: Vec<&Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect();
            if !has_nil || non_nil.len() != 1 {
                return Skip("union receiver has no single non-nil grounding");
            }
            match classify(Some(non_nil[0]), defs) {
                Container { .. } => Container { nilable: true },
                NeverBlank { .. } => NeverBlank { nilable: true },
                // `Bool | Nil`: `!r` and `r ? true : nil` already
                // treat nil and false alike, so plain BoolLike forms
                // stay correct.
                BoolLike => BoolLike,
                AlwaysNil => AlwaysNil,
                OwnDispatch => Skip("nilable receiver of a class with its own predicate"),
                other @ Skip(_) => other,
            }
        }
        Ty::Untyped => Skip("untyped receiver"),
        Ty::Var { .. } => Skip("receiver type unresolved"),
        _ => Skip("receiver type has no blank-predicate grounding"),
    }
}

/// A receiver that is safe to re-evaluate or drop: an effect-free
/// chain of reads. Zero-arg sends ride the analyzer's effect
/// annotations — an AR write (`save`, `destroy`) carries effects and
/// fails this test, a memoized column reader doesn't. Shared purity
/// gate for the hook passes (blank grounding, update-kwargs inlining).
pub(crate) fn is_effect_free_reader(e: &Expr) -> bool {
    if !e.effects.is_pure() {
        return false;
    }
    match &*e.node {
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::SelfRef
        | ExprNode::Const { .. } => true,
        ExprNode::Send { recv, args, block: None, .. } if args.is_empty() => {
            recv.as_ref().map_or(true, is_effect_free_reader)
        }
        // Indexed reads (`params[:preview]`, `cookies[COOKIE]`) are
        // reads all the same — pure receiver + pure index re-evaluate
        // safely. Other with-args sends stay conservative.
        ExprNode::Send { recv, args, block: None, method, .. }
            if method.as_str() == "[]" =>
        {
            recv.as_ref().map_or(true, is_effect_free_reader)
                && args.iter().all(is_effect_free_reader)
        }
        _ => false,
    }
}

fn walk(expr: &mut Expr, defs: &AppDefinitions, diags: &mut Vec<Diagnostic>) {
    // Children first: a rewritten site's receiver subtree never needs
    // revisiting, and inner blank-predicates (e.g. in an interpolation
    // inside a `present?` receiver) are already grounded by the time
    // the outer node is considered.
    match &mut *expr.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, defs, diags);
            }
            for a in args {
                walk(a, defs, diags);
            }
            if let Some(b) = block {
                walk(b, defs, diags);
            }
        }
        ExprNode::Apply { fun, args, block } => {
            walk(fun, defs, diags);
            for a in args {
                walk(a, defs, diags);
            }
            if let Some(b) = block {
                walk(b, defs, diags);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                walk(e, defs, diags);
            }
        }
        ExprNode::Array { elements, .. } => {
            for e in elements {
                walk(e, defs, diags);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, defs, diags);
                walk(v, defs, diags);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr: e } = p {
                    walk(e, defs, diags);
                }
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, defs, diags);
            walk(right, defs, diags);
        }
        ExprNode::Let { value, body, .. } => {
            walk(value, defs, diags);
            walk(body, defs, diags);
        }
        ExprNode::Lambda { body, .. } => walk(body, defs, diags),
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, defs, diags);
            walk(then_branch, defs, diags);
            walk(else_branch, defs, diags);
        }
        ExprNode::Case { scrutinee, arms } => {
            walk(scrutinee, defs, diags);
            for Arm { guard, body, .. } in arms {
                if let Some(g) = guard {
                    walk(g, defs, diags);
                }
                walk(body, defs, diags);
            }
        }
        ExprNode::Assign { target, value } => {
            walk_lvalue(target, defs, diags);
            walk(value, defs, diags);
        }
        ExprNode::OpAssign { target, value, .. } => {
            walk_lvalue(target, defs, diags);
            walk(value, defs, diags);
        }
        ExprNode::MultiAssign { targets, value } => {
            for t in targets {
                walk_lvalue(t, defs, diags);
            }
            walk(value, defs, diags);
        }
        ExprNode::Yield { args } => {
            for a in args {
                walk(a, defs, diags);
            }
        }
        ExprNode::Raise { value } => walk(value, defs, diags),
        ExprNode::RescueModifier { expr: e, fallback } => {
            walk(e, defs, diags);
            walk(fallback, defs, diags);
        }
        ExprNode::Return { value } => walk(value, defs, diags),
        ExprNode::Super { args } => {
            if let Some(args) = args {
                for a in args {
                    walk(a, defs, diags);
                }
            }
        }
        ExprNode::Next { value } | ExprNode::Break { value } => {
            if let Some(v) = value {
                walk(v, defs, diags);
            }
        }
        ExprNode::Splat { value } => walk(value, defs, diags),
        ExprNode::While { cond, body, .. } => {
            walk(cond, defs, diags);
            walk(body, defs, diags);
        }
        ExprNode::Range { begin, end, .. } => {
            if let Some(b) = begin {
                walk(b, defs, diags);
            }
            if let Some(e) = end {
                walk(e, defs, diags);
            }
        }
        ExprNode::BeginRescue { body, rescues, else_branch, ensure, .. } => {
            walk(body, defs, diags);
            for RescueClause { classes, body, .. } in rescues {
                for c in classes {
                    walk(c, defs, diags);
                }
                walk(body, defs, diags);
            }
            if let Some(e) = else_branch {
                walk(e, defs, diags);
            }
            if let Some(e) = ensure {
                walk(e, defs, diags);
            }
        }
        ExprNode::Cast { value, .. } => walk(value, defs, diags),
        ExprNode::Lit { .. }
        | ExprNode::Var { .. }
        | ExprNode::Ivar { .. }
        | ExprNode::Const { .. }
        | ExprNode::SelfRef
        | ExprNode::Retry
        | ExprNode::Redo => {}
    }

    try_rewrite(expr, defs, diags);
}

fn walk_lvalue(target: &mut LValue, defs: &AppDefinitions, diags: &mut Vec<Diagnostic>) {
    match target {
        LValue::Attr { recv, .. } => walk(recv, defs, diags),
        LValue::Index { recv, index } => {
            walk(recv, defs, diags);
            walk(index, defs, diags);
        }
        _ => {}
    }
}

/// The three predicates, owned so no borrow of the Send node outlives
/// the decision phase.
#[derive(Clone, Copy, PartialEq)]
enum Pred {
    Blank,
    Present,
    Presence,
}

fn try_rewrite(expr: &mut Expr, defs: &AppDefinitions, diags: &mut Vec<Diagnostic>) {
    use Grounding::*;

    // Decision phase: everything read out of the node is owned before
    // any mutation.
    let (pred, grounding, recv_ty, pure_recv) = {
        let ExprNode::Send { recv: Some(r), method, args, block, .. } = &*expr.node else {
            return;
        };
        let pred = match method.as_str() {
            "blank?" => Pred::Blank,
            "present?" => Pred::Present,
            "presence" => Pred::Presence,
            _ => return,
        };
        if !args.is_empty() || block.is_some() {
            return;
        }
        (pred, classify(r.ty.as_ref(), defs), r.ty.clone(), is_effect_free_reader(r))
    };
    let method_name = match pred {
        Pred::Blank => "blank?",
        Pred::Present => "present?",
        Pred::Presence => "presence",
    };

    match &grounding {
        OwnDispatch => return,
        Skip(reason) => {
            diags.push(unlowered(expr, recv_ty.as_ref(), method_name, reason));
            return;
        }
        _ => {}
    }

    // Forms that evaluate the receiver more than once (or fold it
    // away) demand an effect-free reader; the single-eval forms
    // (`r.strip.empty?`, `r.nil?`, `!r`, `r ? true : nil`, plain `r`)
    // don't.
    let needs_pure = match (&grounding, pred) {
        (Container { .. }, Pred::Presence) => true,
        (Container { nilable: true }, _) => true,
        (NeverBlank { nilable: false }, Pred::Blank | Pred::Present) => true,
        (AlwaysNil, _) => true,
        _ => false,
    };
    if needs_pure && !pure_recv {
        diags.push(unlowered(
            expr,
            recv_ty.as_ref(),
            method_name,
            "receiver has effects; not safely re-evaluable",
        ));
        return;
    }

    // Take ownership of the receiver; the placeholder node is
    // immediately overwritten below.
    let span = expr.span;
    let leading_blank_line = expr.leading_blank_line;
    let old = std::mem::replace(&mut *expr.node, ExprNode::SelfRef);
    let ExprNode::Send { recv: Some(r), .. } = old else { unreachable!() };

    let replacement = match grounding {
        Container { nilable } => rewrite_emptyable(span, r, pred, nilable),
        NeverBlank { nilable } => rewrite_never_blank(span, r, pred, nilable),
        BoolLike => rewrite_bool(span, r, pred),
        AlwaysNil => match pred {
            Pred::Blank => lit_bool(span, true),
            Pred::Present => lit_bool(span, false),
            Pred::Presence => lit_nil(span),
        },
        OwnDispatch | Skip(_) => unreachable!(),
    };

    *expr = replacement;
    expr.span = span;
    expr.leading_blank_line = leading_blank_line;
}

/// Shared shape for the two `empty?`-style groundings. `empty_form`
/// builds the "is empty" test for a non-nil receiver.
fn rewrite_emptyable(span: crate::span::Span, r: Expr, pred: Pred, nilable: bool) -> Expr {
    let empty_form = plain_empty;
    let value_ty = non_nil_ty(&r);
    match (pred, nilable) {
        (Pred::Blank, false) => empty_form(span, r),
        (Pred::Present, false) => not(span, empty_form(span, r)),
        (Pred::Blank, true) => bool_op(
            span,
            crate::expr::BoolOpKind::Or,
            nil_check(span, r.clone()),
            empty_form(span, r),
        ),
        // `!r.nil? && !r.strip.empty?` — both operands are unary
        // sends, so no `!`-around-`||` precedence hazard reaches any
        // emitter.
        (Pred::Present, true) => bool_op(
            span,
            crate::expr::BoolOpKind::And,
            not(span, nil_check(span, r.clone())),
            not(span, empty_form(span, r)),
        ),
        // presence: `<blank-form> ? nil : r`
        (Pred::Presence, false) => {
            let cond = empty_form(span, r.clone());
            if_expr(span, cond, lit_nil(span), r, nullable(value_ty))
        }
        (Pred::Presence, true) => {
            let cond = bool_op(
                span,
                crate::expr::BoolOpKind::Or,
                nil_check(span, r.clone()),
                empty_form(span, r.clone()),
            );
            if_expr(span, cond, lit_nil(span), r, nullable(value_ty))
        }
    }
}

fn rewrite_never_blank(span: crate::span::Span, r: Expr, pred: Pred, nilable: bool) -> Expr {
    match (pred, nilable) {
        // Purity was already required for the folds.
        (Pred::Blank, false) => lit_bool(span, false),
        (Pred::Present, false) => lit_bool(span, true),
        (Pred::Blank, true) => nil_check(span, r),
        (Pred::Present, true) => not(span, nil_check(span, r)),
        // presence of a never-blank value is the value itself —
        // nil stays nil, everything else is present.
        (Pred::Presence, _) => r,
    }
}

fn rewrite_bool(span: crate::span::Span, r: Expr, pred: Pred) -> Expr {
    match pred {
        // `false.blank?` is true in AS; `!r` also sends nil (the
        // nilable case) to true.
        Pred::Blank => not(span, r),
        Pred::Present => {
            let inner = not(span, r);
            not(span, inner)
        }
        // presence: only `true` is present, so the kept value is the
        // literal — single evaluation.
        Pred::Presence => {
            let value_ty = nullable(Ty::Bool);
            if_expr(span, r, lit_bool(span, true), lit_nil(span), value_ty)
        }
    }
}

// ---- node builders ------------------------------------------------------

fn mk(span: crate::span::Span, node: ExprNode, ty: Ty) -> Expr {
    let mut e = Expr::new(span, node);
    e.ty = Some(ty);
    e
}

fn send0(span: crate::span::Span, recv: Expr, name: &str, ty: Ty) -> Expr {
    mk(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::new(name),
            args: vec![],
            block: None,
            parenthesized: false,
        },
        ty,
    )
}

fn plain_empty(span: crate::span::Span, r: Expr) -> Expr {
    send0(span, r, "empty?", Ty::Bool)
}

fn nil_check(span: crate::span::Span, r: Expr) -> Expr {
    send0(span, r, "nil?", Ty::Bool)
}

fn not(span: crate::span::Span, e: Expr) -> Expr {
    send0(span, e, "!", Ty::Bool)
}

fn bool_op(span: crate::span::Span, op: crate::expr::BoolOpKind, l: Expr, r: Expr) -> Expr {
    mk(
        span,
        ExprNode::BoolOp { op, surface: Default::default(), left: l, right: r },
        Ty::Bool,
    )
}

fn if_expr(span: crate::span::Span, cond: Expr, t: Expr, e: Expr, ty: Ty) -> Expr {
    mk(span, ExprNode::If { cond, then_branch: t, else_branch: e }, ty)
}

fn lit_bool(span: crate::span::Span, value: bool) -> Expr {
    mk(span, ExprNode::Lit { value: Literal::Bool { value } }, Ty::Bool)
}

fn lit_nil(span: crate::span::Span) -> Expr {
    mk(span, ExprNode::Lit { value: Literal::Nil }, Ty::Nil)
}

/// The receiver's type with the `Nil` variant stripped — what
/// `presence` yields on the kept branch.
fn non_nil_ty(r: &Expr) -> Ty {
    match r.ty.as_ref() {
        Some(Ty::Union { variants }) => {
            let non_nil: Vec<Ty> =
                variants.iter().filter(|v| !matches!(v, Ty::Nil)).cloned().collect();
            match non_nil.len() {
                1 => non_nil.into_iter().next().unwrap(),
                _ => Ty::Union { variants: non_nil },
            }
        }
        Some(t) => t.clone(),
        None => Ty::Untyped,
    }
}

fn nullable(t: Ty) -> Ty {
    crate::analyze::union_of(t, Ty::Nil)
}

fn unlowered(expr: &Expr, recv_ty: Option<&Ty>, method: &str, reason: &str) -> Diagnostic {
    let recv_ty = recv_ty.cloned().unwrap_or(Ty::Untyped);
    let kind = DiagnosticKind::BlankUnlowered {
        method: Symbol::new(method),
        recv_ty: recv_ty.clone(),
        reason: Symbol::new(reason),
    };
    Diagnostic {
        span: expr.span,
        severity: Diagnostic::default_severity(&kind),
        kind,
        message: format!(
            "`{method}` on receiver typed {recv_ty:?} left as dynamic dispatch ({reason}) — \
             the CRuby overlay serves it at runtime; AOT/strict targets cannot compile it"
        ),
    }
}
