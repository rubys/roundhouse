//! `Model.authenticate_by(email_address: …, password: …)` (Rails 7.1)
//! grounding: macro-inline the find-then-check at the call site.
//!
//!   __auth = <recv>.find_by(<identifier keys>)
//!   if __auth.nil?
//!     nil
//!   else
//!     if __auth.authenticate(<password value>)
//!       nil
//!     else
//!       __auth = nil
//!     end
//!   end
//!   <the original expression position> → __auth
//!
//! Two unmodeled things meet at campfire's `User.active
//! .authenticate_by(…)`, and the expansion dissolves both at once.
//!
//! **The API.** Rails partitions the keyword hash at runtime: a key the
//! model has no attribute for but does have a `<key>_digest` for is a
//! password to verify, every other key is a finder condition; the
//! record comes back only if the finder matched AND every password
//! verified. That partition is a compile-time fact here — the model's
//! `has_secure_password` declarations name the password keys — so the
//! call expands into surface that is already modeled end to end
//! (`find_by`, and the `authenticate` that
//! [`super::secure_password`] synthesizes), rather than becoming a new
//! runtime method with a heterogeneous hash argument. Same reasoning as
//! `first_or_create`: a mixed-key options hash is the macro-inline
//! case, not the runtime-helper case.
//!
//! **The receiver.** `User.active` is a `Relation[User]`, and a
//! Relation only delegates the *relation-returning* class-method
//! surface on purpose — forwarding arbitrary class methods is Rails'
//! `method_missing`, which this compiler does not model. Expanding at
//! the call site means no class method has to be reached through the
//! Relation at all: `find_by` is catalog surface on BOTH a Class and a
//! Relation receiver, so the same expansion types either way.
//!
//! The receiver is bound ONCE (it is a query), which is why this is a
//! statement-hoisting pass and not a plain expression rewrite: the IR
//! has no safe-navigation node, and its `&.` desugar duplicates the
//! receiver.
//!
//! Type-gated on the receiver resolving to a model that declares
//! `has_secure_password`; anything else — an unknown receiver, a
//! non-literal key, a hash with no password key or no finder key (which
//! Rails itself rejects with ArgumentError) — is left in source shape
//! and goes on the residue ledger.
//!
//! Runs on the post-analyze hook (`apply_post_analyze_lowerings`) so
//! every target consumes the grounded form, and so the dispatch-failure
//! diagnostic `diagnose` would otherwise raise for `authenticate_by`
//! resolves instead of accumulating as phantom modeling debt.

use std::collections::HashMap;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// The bound receiver-record local. One name is enough: each expansion
/// is consumed inside the statement it was hoisted above.
const TEMP: &str = "__auth";

/// Inline `authenticate_by` sends across every hook body. Returns the
/// residue ledger: recognized-by-name calls left in source shape.
pub fn apply_authenticate_by_lowering(app: &mut App) -> Vec<Diagnostic> {
    let secure: HashMap<ClassId, Vec<Symbol>> = app
        .models
        .iter()
        .filter_map(|m| {
            let attrs = super::secure_password::secure_password_attrs(&m.body);
            (!attrs.is_empty()).then(|| (m.name.clone(), attrs))
        })
        .collect();
    super::for_each_hook_body(app, &mut |body| rewrite_body(body, &secure));
    let mut diags = Vec::new();
    super::for_each_hook_body_ref(app, &mut |body| collect_residue(body, &secure, &mut diags));
    diags
}

fn rewrite_body(body: &mut Expr, secure: &HashMap<ClassId, Vec<Symbol>>) {
    rewrite(body, secure);
    // A body whose whole source is one statement has no `Seq` to hoist
    // into — campfire's `def create` is exactly `if user =
    // User.active.authenticate_by(…)`. The body root can BECOME the
    // Seq; an interior expression cannot (a Seq in a value slot emits
    // as newline-joined statements, which is broken Ruby).
    if !matches!(&*body.node, ExprNode::Seq { .. }) {
        if let Some(prelude) = take_claim(body, secure) {
            hoist(body, prelude);
        }
    }
}

fn rewrite(expr: &mut Expr, secure: &HashMap<ClassId, Vec<Symbol>>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, secure));
    let ExprNode::Seq { exprs } = &mut *expr.node else { return };
    for e in exprs {
        if let Some(prelude) = take_claim(e, secure) {
            hoist(e, prelude);
        }
    }
}

/// Replace the statement with `prelude…; <statement>` — the statement
/// having already had the claimed call swapped for a `__auth` read.
fn hoist(stmt: &mut Expr, mut prelude: Vec<Expr>) {
    prelude.push(stmt.clone());
    *stmt.node = ExprNode::Seq { exprs: prelude };
    stmt.ty = None;
}

/// Find the first claimable `authenticate_by` in an EAGERLY evaluated
/// position of `stmt`, swap it for a `__auth` read, and return the
/// statements that must run before `stmt`. Descends only through slots
/// that always evaluate exactly once when the statement runs — a call
/// under a block, a lambda, an `if` branch, a loop or the right operand
/// of `&&` would change meaning if hoisted out, so those are left alone
/// (and reported as residue).
fn take_claim(e: &mut Expr, secure: &HashMap<ClassId, Vec<Symbol>>) -> Option<Vec<Expr>> {
    if let Some(plan) = claims(e, secure) {
        let prelude = build(&plan, e.span);
        *e.node = ExprNode::Var { id: VarId(0), name: Symbol::from(TEMP) };
        e.ty = Some(optional(&plan.class));
        return Some(prelude);
    }
    match &mut *e.node {
        ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => {
            take_claim(value, secure)
        }
        ExprNode::If { cond, .. } => take_claim(cond, secure),
        ExprNode::Case { scrutinee, .. } => take_claim(scrutinee, secure),
        ExprNode::Return { value } | ExprNode::Raise { value } => take_claim(value, secure),
        ExprNode::BoolOp { left, .. } => take_claim(left, secure),
        ExprNode::Send { recv, args, .. } => recv
            .as_mut()
            .and_then(|r| take_claim(r, secure))
            .or_else(|| args.iter_mut().find_map(|a| take_claim(a, secure))),
        _ => None,
    }
}

/// What one claimed call expands into.
struct Plan {
    recv: Expr,
    class: ClassId,
    /// Keys naming no secure-password attribute — the `find_by`
    /// conditions.
    identifiers: Vec<(Symbol, Expr)>,
    /// Keys naming a secure-password attribute, paired with the
    /// plaintext to verify.
    passwords: Vec<(Symbol, Expr)>,
}

fn claims(e: &Expr, secure: &HashMap<ClassId, Vec<Symbol>>) -> Option<Plan> {
    let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "authenticate_by" || args.len() != 1 {
        return None;
    }
    let class = match r.ty.as_ref()? {
        Ty::Class { id, .. } => id.clone(),
        Ty::Relation { of } => of.clone(),
        _ => return None,
    };
    let attrs = secure.get(&class)?;
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
    let mut identifiers = Vec::new();
    let mut passwords = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
            return None;
        };
        if attrs.contains(name) {
            passwords.push((name.clone(), v.clone()));
        } else {
            identifiers.push((name.clone(), v.clone()));
        }
    }
    // Rails raises ArgumentError for either half missing; a call that
    // would raise is not one to expand.
    (!identifiers.is_empty() && !passwords.is_empty()).then(|| Plan {
        recv: r.clone(),
        class,
        identifiers,
        passwords,
    })
}

fn optional(class: &ClassId) -> Ty {
    Ty::Union {
        variants: vec![Ty::Class { id: class.clone(), args: vec![] }, Ty::Nil],
    }
}

fn build(plan: &Plan, span: Span) -> Vec<Expr> {
    let self_ty = Ty::Class { id: plan.class.clone(), args: vec![] };
    let typed = |node: ExprNode, ty: Ty| {
        let mut e = Expr::new(span, node);
        e.ty = Some(ty);
        e
    };
    let temp = |ty: &Ty| {
        typed(ExprNode::Var { id: VarId(0), name: Symbol::from(TEMP) }, ty.clone())
    };
    let assign_temp = |value: Expr| {
        Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Var { id: VarId(0), name: Symbol::from(TEMP) },
                value,
            },
        )
    };
    let nil = || Expr::new(span, ExprNode::Lit { value: Literal::Nil });

    let conditions = Expr::new(
        span,
        ExprNode::Hash {
            entries: plan
                .identifiers
                .iter()
                .map(|(k, v)| {
                    (
                        Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: k.clone() } }),
                        v.clone(),
                    )
                })
                .collect(),
            kwargs: true,
        },
    );
    let find = typed(
        ExprNode::Send {
            recv: Some(plan.recv.clone()),
            method: Symbol::from("find_by"),
            args: vec![conditions],
            block: None,
            parenthesized: true,
        },
        optional(&plan.class),
    );

    let found = typed(
        ExprNode::Send {
            recv: Some(temp(&optional(&plan.class))),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
        Ty::Bool,
    );

    // Every declared password must verify. `authenticate` answers the
    // record or `false`, so the calls compose with `&&` directly —
    // the same truthiness the surface API is built on.
    let mut check: Option<Expr> = None;
    for (attr, plaintext) in &plan.passwords {
        let call = typed(
            ExprNode::Send {
                recv: Some(temp(&self_ty)),
                method: super::secure_password::authenticator_name(attr),
                args: vec![plaintext.clone()],
                block: None,
                parenthesized: true,
            },
            self_ty.clone(),
        );
        check = Some(match check {
            None => call,
            Some(prev) => typed(
                ExprNode::BoolOp {
                    op: BoolOpKind::And,
                    surface: BoolOpSurface::Symbol,
                    left: prev,
                    right: call,
                },
                self_ty.clone(),
            ),
        });
    }

    let verify = Expr::new(
        span,
        ExprNode::If {
            cond: check.expect("claims() requires at least one password key"),
            then_branch: nil(),
            else_branch: assign_temp(nil()),
        },
    );
    let guard = Expr::new(
        span,
        ExprNode::If { cond: found, then_branch: nil(), else_branch: verify },
    );
    vec![assign_temp(find), guard]
}

/// Every `authenticate_by` still standing after the rewrite — a name
/// nothing else in the pipeline models, so each survivor is a real gap
/// and says which gate it failed.
fn collect_residue(
    e: &Expr,
    secure: &HashMap<ClassId, Vec<Symbol>>,
    out: &mut Vec<Diagnostic>,
) {
    if matches!(&*e.node, ExprNode::Send { method, .. } if method.as_str() == "authenticate_by") {
        let reason = residue_reason(e, secure);
        out.push(super::residue_diagnostic(
            "authenticate_by",
            "authenticate_by",
            e.span,
            reason,
            format!(
                "`authenticate_by` left uninlined ({reason}) — no target models \
                 it as a method, so the call has no runtime home"
            ),
        ));
    }
    e.node.for_each_child(&mut |c| collect_residue(c, secure, out));
}

/// Which gate the surviving call failed, in the order `claims` applies
/// them — the ledger entry is only useful if it names the actual reason.
fn residue_reason(e: &Expr, secure: &HashMap<ClassId, Vec<Symbol>>) -> &'static str {
    let ExprNode::Send { recv: Some(r), args, block, .. } = &*e.node else {
        return "call has no receiver to resolve a model from";
    };
    if block.is_some() {
        return "call takes a block";
    }
    let class = match r.ty.as_ref() {
        Some(Ty::Class { id, .. }) => id.clone(),
        Some(Ty::Relation { of }) => of.clone(),
        _ => return "receiver does not resolve to a model or a relation over one",
    };
    let Some(attrs) = secure.get(&class) else {
        return "receiver's model does not declare has_secure_password";
    };
    let entries = match args.first().map(|a| &*a.node) {
        Some(ExprNode::Hash { entries, .. }) if args.len() == 1 => entries,
        _ => return "arguments are not a single hash literal",
    };
    let mut identifiers = 0;
    let mut passwords = 0;
    for (k, _) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: name } } = &*k.node else {
            return "hash has a non-literal key";
        };
        if attrs.contains(name) {
            passwords += 1;
        } else {
            identifiers += 1;
        }
    }
    if passwords == 0 {
        return "hash names no secure-password attribute";
    }
    if identifiers == 0 {
        return "hash has no finder key";
    }
    "call site is not an eagerly evaluated statement position"
}
