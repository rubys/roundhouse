//! `direct :name do |…| route_for :target, … end` → a real
//! `RouteHelpers.<name>_path` function.
//!
//! `direct` is a custom URL helper, not a route: it adds nothing to the
//! dispatch table, it names a builder whose body is arbitrary Ruby.
//! Ingest used to drop it with a ledger line, so campfire's layout
//! called `fresh_account_logo_path` and got a NameError on every page.
//!
//! ## The calling convention
//!
//! Rails invokes the block with the helper's arguments PLUS a trailing
//! options hash, always. `fresh_account_logo_path` (no arguments) calls
//! `do |options|` with `({})`; `fresh_user_avatar_path(user)` calls
//! `do |user, options|` with `(user, {})`. So the LAST block parameter
//! is the options hash, and giving it a `{}` default is what makes the
//! no-argument call sites work.
//!
//! ## `route_for`
//!
//! `route_for :target, arg…, k: v` resolves to the target's own path
//! helper with the positional args, plus a query string built from the
//! keywords. Three rules, all MEASURED against Rails 8.1 rather than
//! assumed (`direct` + `url_for` + `Hash#to_query` compose in ways that
//! are hard to predict):
//!
//! ```text
//! route_for :user_avatar, "tok", v: "123", size: nil  → /users/tok/avatar?v=123
//! route_for :user_avatar, "tok", v: "123", size: 48   → /users/tok/avatar?size=48&v=123
//! ```
//!
//!   * a nil-valued key is DROPPED, not rendered as `size=`;
//!   * keys are sorted ALPHABETICALLY, not written order — so
//!     `v:` before `size:` in the source still emits `size` first;
//!   * positional args fill the target path's segments.
//!
//! The keys are static, so the sort happens at COMPILE time and the
//! emitted body only has to handle the nil-dropping — which it does with
//! a `Vec<String>` of rendered `k=v` pieces, joined with `&`. A concrete
//! `Array[String]` rather than passing the option hash to some runtime
//! query-builder: that would be a type-erased bag, and every target
//! already handles array push/join.

use crate::App;
use crate::dialect::{DirectHelper, LibraryFunction, Param};
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;

use super::super::routes::FlatRoute;

/// One `RouteHelpers.<name>_path` per `direct` declaration.
/// `helper_params`: each generated route helper's parameter types, by
/// name, so a `route_for` argument can meet its target's signature.
pub fn lower_direct_helpers(
    module_path: &[Symbol],
    app: &App,
    flat: &[FlatRoute],
    helper_params: &crate::det::HashMap<String, Vec<crate::ty::Ty>>,
) -> Vec<LibraryFunction> {
    app.routes
        .direct_helpers
        .iter()
        .map(|h| build_direct_helper(module_path, h, flat, helper_params))
        .collect()
}

fn build_direct_helper(
    module_path: &[Symbol],
    helper: &DirectHelper,
    flat: &[FlatRoute],
    helper_params: &crate::det::HashMap<String, Vec<crate::ty::Ty>>,
) -> LibraryFunction {
    // The trailing options hash takes a `{}` default — see the header.
    let last = helper.params.len().saturating_sub(1);
    let params: Vec<Param> = helper
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if i == last {
                Param::with_default(p.clone(), empty_hash())
            } else {
                Param::positional(p.clone())
            }
        })
        .collect();
    let mut body = helper.body.clone();
    // The temporaries `rewrite_route_for` introduces take names the
    // helper doesn't already use: its block parameters and its locals.
    let mut taken: crate::det::HashSet<Symbol> = helper.params.iter().cloned().collect();
    crate::lower::model_to_library::markers::collect_local_names(&body, &mut taken);
    let fresh = FreshLocals { next: std::cell::Cell::new(0), taken: std::cell::RefCell::new(taken) };
    rewrite_route_for(&mut body, flat, helper_params, &fresh);
    LibraryFunction {
        module_path: module_path.to_vec(),
        name: Symbol::from(format!("{}_path", helper.name.as_str())),
        params,
        unsupported_formals: None,
        has_anonymous_block: false,
        body,
        signature: None,
        effects: Default::default(),
        is_async: false,
    }
}

fn empty_hash() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Hash { entries: vec![], kwargs: false })
}

/// `route_for :target, arg…, k: v` → `<target>_path(arg…)` + query.
fn rewrite_route_for(
    expr: &mut Expr,
    flat: &[FlatRoute],
    helper_params: &crate::det::HashMap<String, Vec<crate::ty::Ty>>,
    fresh: &FreshLocals,
) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite_route_for(c, flat, helper_params, fresh));

    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "route_for" || args.is_empty() {
        return;
    }
    let ExprNode::Lit { value: Literal::Sym { value: target } } = &*args[0].node else {
        return;
    };
    // Split the rest into positional path arguments and the trailing
    // keyword hash, which is the query.
    let rest = &args[1..];
    let (positional, query): (&[Expr], Vec<(Expr, Expr)>) = match rest.last() {
        Some(last) => match &*last.node {
            ExprNode::Hash { entries, .. } => (&rest[..rest.len() - 1], entries.clone()),
            _ => (rest, vec![]),
        },
        None => (rest, vec![]),
    };

    let helper = format!("{}_path", target.as_str());
    // A `route_for` naming a route that does not exist would emit a call
    // to a helper nobody defines. Leave it alone so the failure names the
    // missing route rather than appearing as a mystery NameError.
    if !flat.iter().any(|r| r.named && format!("{}_path", r.as_name) == helper) {
        return;
    }
    let span = expr.span;
    // An Integer into a segment the helper types `String` (a call site
    // elsewhere fills it with a String) is its `to_s`, as Rails writes
    // it with `to_param` and as the other call sites' rewrite does. A
    // record and an Integer segment stay as they are.
    let declared = helper_params.get(&helper);
    let positional: Vec<Expr> = positional
        .iter()
        .enumerate()
        .map(|(i, arg)| {
            // An optional segment (`/archive(/:period)`) is declared
            // `String | nil`: a String slot too.
            let slot = declared.and_then(|tys| tys.get(i));
            let wants_str = matches!(slot.map(crate::ty::Ty::peel_nilable), Some(crate::ty::Ty::Str));
            let is_int = matches!(arg.ty, Some(crate::ty::Ty::Int))
                || matches!(&*arg.node, ExprNode::Lit { value: Literal::Int { .. } });
            // An Integer that can be nil, into an optional segment, keeps
            // its nil (Rails drops the segment) and converts otherwise. A
            // required segment is left as it was: Rails raises on nil there.
            let optional_slot = matches!(slot, Some(crate::ty::Ty::Union { variants }) if variants.contains(&crate::ty::Ty::Nil));
            let nilable_int = matches!(&arg.ty, Some(crate::ty::Ty::Union { variants })
                if variants.contains(&crate::ty::Ty::Int)
                    && variants.iter().all(|v| matches!(v, crate::ty::Ty::Int | crate::ty::Ty::Nil)));
            if wants_str && is_int {
                to_s(arg.clone(), arg.span)
            } else if wants_str && optional_slot && nilable_int {
                to_s_keeping_nil(arg.clone(), fresh)
            } else {
                arg.clone()
            }
        })
        .collect();
    let base = route_helpers_call(&helper, positional, span);
    *expr = with_query(base, &query, span);
}

/// `RouteHelpers.<helper>(args…)`.
fn route_helpers_call(helper: &str, args: Vec<Expr>, span: Span) -> Expr {
    let recv = Expr::new(
        span,
        ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
    );
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(helper),
            args,
            block: None,
            parenthesized: true,
        },
    )
}

/// `base` with the query string appended, as a statement sequence in
/// value position.
///
/// Emits, for keys sorted at COMPILE time:
///
/// ```ruby
/// __q = []
/// __q0 = <size expr>
/// __q.push("size=" + ActionView::ViewHelpers.url_encode(__q0.to_s)) unless __q0.nil?
/// __q1 = <v expr>
/// __q.push("v=" + ActionView::ViewHelpers.url_encode(__q1.to_s)) unless __q1.nil?
/// __qbase = RouteHelpers.account_logo_path
/// __q.empty? ? __qbase : __qbase + "?" + __q.join("&")
/// ```
///
/// Each value lands in a temporary first so the nil guard and the
/// rendered piece share ONE evaluation — campfire's
/// `Current.account&.updated_at&.to_fs(:number)` is a chain the
/// safe-navigation desugar already expands, and testing it twice would
/// run it twice. The base is a temporary for the same reason: it is
/// named in both arms of the final conditional.
///
/// Returns `base` untouched when there are no query keys.
fn with_query(base: Expr, query: &[(Expr, Expr)], span: Span) -> Expr {
    let mut pairs: Vec<(String, Expr)> = query
        .iter()
        .filter_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => {
                Some((value.as_str().to_string(), v.clone()))
            }
            ExprNode::Lit { value: Literal::Str { value } } => Some((value.clone(), v.clone())),
            _ => None,
        })
        .collect();
    if pairs.is_empty() {
        return base;
    }
    // Rails' `Hash#to_query` sorts; written order is not emitted order.
    pairs.sort_by(|a, b| a.0.cmp(&b.0));

    let q = Symbol::from("__q");
    let q_var = || var(&q, span);
    let mut stmts: Vec<Expr> = vec![assign(
        &q,
        Expr::new(span, ExprNode::Array { elements: vec![], style: Default::default() }),
        span,
    )];
    for (i, (key, value)) in pairs.iter().enumerate() {
        let tmp = Symbol::from(format!("__q{i}"));
        stmts.push(assign(&tmp, value.clone(), span));
        let piece = concat(
            lit_str(format!("{key}="), span),
            view_helpers_call("url_encode", vec![to_s(var(&tmp, span), span)], span),
            span,
        );
        let push = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(q_var()),
                method: Symbol::from("push"),
                args: vec![piece],
                block: None,
                parenthesized: true,
            },
        );
        // `unless <value>.nil?` — Rails drops a nil-valued key entirely.
        stmts.push(Expr::new(
            span,
            ExprNode::If {
                cond: send0(var(&tmp, span), "nil?", span),
                then_branch: lit_str(String::new(), span),
                else_branch: push,
            },
        ));
    }
    let base_name = Symbol::from("__qbase");
    stmts.push(assign(&base_name, base, span));
    let joined = concat(
        concat(var(&base_name, span), lit_str("?".to_string(), span), span),
        Expr::new(
            span,
            ExprNode::Send {
                recv: Some(q_var()),
                method: Symbol::from("join"),
                args: vec![lit_str("&".to_string(), span)],
                block: None,
                parenthesized: true,
            },
        ),
        span,
    );
    stmts.push(Expr::new(
        span,
        ExprNode::If {
            cond: send0(q_var(), "empty?", span),
            then_branch: var(&base_name, span),
            else_branch: joined,
        },
    ));
    Expr::new(span, ExprNode::Seq { exprs: stmts })
}

fn var(name: &Symbol, span: Span) -> Expr {
    Expr::new(span, ExprNode::Var { id: VarId(0), name: name.clone() })
}

fn assign(name: &Symbol, value: Expr, span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { name: name.clone(), id: VarId(0) },
            value,
        },
    )
}

fn lit_str(s: String, span: Span) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s } });
    e.ty = Some(crate::ty::Ty::Str);
    e
}

fn send0(recv: Expr, method: &str, span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    )
}

/// Names for the temporaries one direct helper's rewrite introduces:
/// `route_for_arg_<n>`, skipping any name the helper already uses.
struct FreshLocals {
    next: std::cell::Cell<usize>,
    taken: std::cell::RefCell<crate::det::HashSet<Symbol>>,
}

impl FreshLocals {
    fn name(&self) -> Symbol {
        loop {
            let n = self.next.get();
            self.next.set(n + 1);
            let candidate = Symbol::from(format!("route_for_arg_{n}"));
            if self.taken.borrow_mut().insert(candidate.clone()) {
                return candidate;
            }
        }
    }
}

/// `value`'s `to_s`, or nil when it is nil: bound once to a fresh local,
/// then `nil? ? nil : to_s`.
fn to_s_keeping_nil(value: Expr, fresh: &FreshLocals) -> Expr {
    use crate::ty::Ty;
    let span = value.span;
    let name = fresh.name();
    let value_ty = value.ty.clone().unwrap_or(Ty::Untyped);
    let typed = |node: ExprNode, ty: Ty| Expr { ty: Some(ty), ..Expr::new(span, node) };
    let read = || typed(ExprNode::Var { id: VarId(0), name: name.clone() }, value_ty.clone());
    let opt_str = Ty::Union { variants: vec![Ty::Str, Ty::Nil].into() };
    let mut is_nil = send0(read(), "nil?", span);
    is_nil.ty = Some(Ty::Bool);
    let check = typed(
        ExprNode::If {
            cond: is_nil,
            then_branch: typed(ExprNode::Lit { value: Literal::Nil }, Ty::Nil),
            else_branch: to_s(read(), span),
        },
        opt_str.clone(),
    );
    typed(ExprNode::Let { id: VarId(0), name, value, body: check }, opt_str)
}

fn to_s(e: Expr, span: Span) -> Expr {
    let mut out = send0(e, "to_s", span);
    out.ty = Some(crate::ty::Ty::Str);
    out
}

fn view_helpers_call(method: &str, args: Vec<Expr>, span: Span) -> Expr {
    let recv = Expr::new(
        span,
        ExprNode::Const {
            path: vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")],
        },
    );
    let mut e = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(method),
            args,
            block: None,
            parenthesized: true,
        },
    );
    e.ty = Some(crate::ty::Ty::Str);
    e
}

/// `<a> + <b>`, stamped String — `+` rather than `<<`, per the
/// frozen-literal lesson in `lower::literal_append`.
fn concat(a: Expr, b: Expr, span: Span) -> Expr {
    let mut e = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(a),
            method: Symbol::from("+"),
            args: vec![b],
            block: None,
            parenthesized: false,
        },
    );
    e.ty = Some(crate::ty::Ty::Str);
    e
}

/// Interpolation helper kept for symmetry with the other builders; the
/// query path uses `+` concatenation so the pieces stay String-typed.
#[allow(dead_code)]
fn interp(parts: Vec<InterpPart>, span: Span) -> Expr {
    let mut e = Expr::new(span, ExprNode::StringInterp { parts });
    e.ty = Some(crate::ty::Ty::Str);
    e
}
