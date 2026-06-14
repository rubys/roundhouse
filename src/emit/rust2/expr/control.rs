//! Control-flow emit — If/Else, While/Until, Case/match, Seq, Return,
//! BoolOp (`&&`/`||`). Each function consumes the relevant IR
//! sub-fields and produces a self-contained Rust expression string.
//! The `Seq` emitter is also where the two Option-narrowing fusions
//! (`let-else` and standalone-guard-unwrap) fire, since they look at
//! adjacent statements.

use crate::expr::{Expr, ExprNode, LValue, Literal};

use super::util::{arm_body_already_value, emit_case_pattern, indent, peel_nil};
use super::{
    current_return_is_option, current_return_is_unit, current_return_ty,
    emit_expr, emit_expr_tail, in_constructor, in_return_tail, mark_rebound_var,
    render_self_literal, with_declared_vars_scope, with_rebound_vars_scope,
};

/// Wrap a branch's emitted body in `{ … }` only when the IR shape is
/// a multi-statement `Seq` — i.e. when Rust's parser actually needs
/// the block delimiter to absorb the statement list. Single-expr
/// branches drop the braces (rustc would otherwise flag
/// `unused_braces`). Stage 1 of #22.
fn wrap_as_block_if_multi(branch: &Expr, emitted: String) -> String {
    let multi = matches!(&*branch.node, ExprNode::Seq { exprs } if exprs.len() > 1);
    if multi {
        format!("{{ {emitted} }}")
    } else {
        emitted
    }
}

pub(super) fn emit_if(cond: &Expr, then_branch: &Expr, else_branch: &Expr) -> String {
    // Ruby `cond ? a : b` and `if cond; a; else b; end` both lower to
    // `ExprNode::If`. The lowerer also produces this shape for the
    // modifier forms `STMT if COND` / `STMT unless COND`, with the
    // absent else branch synthesized as `Nil`. Two cases trigger
    // statement-form `if cond { ... }` (no else clause):
    //   1. then diverges (Return/Raise) AND else is Nil — the else is
    //      dead code after the diverging then.
    //   2. else is Nil — `errors << "msg" if cond` style. Implicit
    //      else=nil in Ruby returns nil; in Rust the statement form
    //      returns `()`, which matches the void-context default.
    // Both-branches-present cases (ternary, `if/else/end` with non-Nil
    // else) keep the expression form.
    let else_is_nil = matches!(
        &*else_branch.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    let then_is_nil = matches!(
        &*then_branch.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    if else_is_nil {
        // In the tail position of an `Option<T>`-returning function,
        // emit `if X { Some(Y) } else { None }` so the if-expression's
        // type matches the function return. Otherwise emit the
        // statement-form `if X { Y }` (returns `()`, OK for void
        // statement context).
        //
        // `Some({ then_s })` instead of `Some(then_s)` so a
        // multi-statement Seq branch parses: `Some` takes an
        // expression and a bare statement list isn't one, but a `{ … }`
        // block evaluating to its tail expression is. Single-expr
        // branches don't need the braces — skip them so rustc's
        // `unused_braces` lint stays clean (Stage 1 of #22).
        let cond_s = emit_expr(cond);
        let then_s = with_declared_vars_scope(|| emit_expr_tail(then_branch));
        let then_wrapped = wrap_as_block_if_multi(then_branch, then_s);
        if in_return_tail() && current_return_is_option() {
            // Skip the Some wrap when the inner branch already
            // produces an `Option<T>` — the `none_init_option_return_ty`
            // back-prop typed `let mut result: Option<T> = None` so the
            // Seq's tail Var read already produces `Option<T>`.
            if tail_produces_option(then_branch) {
                return format!("if {cond_s} {{ {then_wrapped} }} else {{ None }}");
            }
            return format!("if {cond_s} {{ Some({then_wrapped}) }} else {{ None }}");
        }
        return format!("if {cond_s} {{ {then_wrapped} }}");
    }
    // `STMT unless COND` lowers to `If { cond, then: Nil, else: STMT }`
    // — emit as the negated single-branch form so the Nil-vs-Assign
    // branch mismatch (E0308 "if and else have incompatible types")
    // doesn't surface.
    if then_is_nil {
        let cond_s = emit_expr(cond);
        let else_s = with_declared_vars_scope(|| emit_expr_tail(else_branch));
        let else_wrapped = wrap_as_block_if_multi(else_branch, else_s);
        if in_return_tail() && current_return_is_option() {
            if tail_produces_option(else_branch) {
                return format!("if !({cond_s}) {{ {else_wrapped} }} else {{ None }}");
            }
            return format!("if !({cond_s}) {{ Some({else_wrapped}) }} else {{ None }}");
        }
        return format!("if !({cond_s}) {{ {else_wrapped} }}");
    }
    // Per-branch DECLARED_VARS scope: each branch's body is a separate
    // Rust scope, so a `let json = X` in one branch doesn't carry the
    // binding into the other branch or the statements after the if.
    let cond_s = emit_expr(cond);
    let then_s = with_declared_vars_scope(|| emit_expr_tail(then_branch));
    let else_s = with_declared_vars_scope(|| emit_expr_tail(else_branch));
    format!("if {cond_s} {{ {then_s} }} else {{ {else_s} }}")
}

pub(super) fn emit_while(cond: &Expr, body: &Expr, until_form: bool) -> String {
    // Rust has no `until`; rewrite to `while !cond` for parity.
    let cond_s = emit_expr(cond);
    let body_s = emit_expr(body);
    let cond_clause = if until_form {
        format!("!({cond_s})")
    } else {
        cond_s
    };
    format!("while {cond_clause} {{\n{}\n}}", indent(&body_s, 1))
}

pub(super) fn emit_seq(exprs: &[Expr]) -> String {
    with_rebound_vars_scope(|| {
        // Rust statements are `;`-terminated; the last expression is
        // the block's value (no trailing `;`). The tail expression
        // inherits the enclosing function's return-tail flag.
        let mut lines = Vec::with_capacity(exprs.len());
        let last = exprs.len().saturating_sub(1);
        let mut i = 0;
        while i < exprs.len() {
            // Guard-clause let-else fusion: detect
            //   let x = OPT;
            //   if x.nil? { return nil };  (or raise, etc.)
            //   ... uses of x narrowed to non-nil ...
            // and emit as
            //   let Some(x) = OPT else { return None };
            // The body-typer narrows `x` for the subsequent statements,
            // but `let mut x = OPT` in Rust still types as `Option<T>`
            // — the let-else form hands Rust the same narrowing.
            if i + 1 <= last {
                if let Some((name, rendered)) = try_fuse_let_else(&exprs[i], &exprs[i + 1]) {
                    mark_rebound_var(&name);
                    lines.push(format!("{rendered};"));
                    i += 2;
                    continue;
                }
            }
            // Standalone guard-clause unwrap: a Seq stmt of the form
            // `if x.nil? { return Y }` where `x` is a Var. Rewrite to
            // `let Some(x) = x else { return Y; };` — rebinds `x` to
            // the unwrapped value.
            if let Some((name, rendered)) = try_emit_param_guard_unwrap(&exprs[i]) {
                mark_rebound_var(&name);
                lines.push(format!("{rendered};"));
                i += 1;
                continue;
            }
            let e = &exprs[i];
            // Trailing `nil` in a void-return Ruby method: drop. Lit::Nil
            // emits as `None` (Option::None constructor), which fails
            // E0308 in a function declared `-> ()`. Rust functions
            // implicitly return `()` at the end of a block.
            if i == last
                && current_return_is_unit()
                && matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
            {
                if !lines.is_empty() {
                    let last_line = lines.last_mut().unwrap();
                    if !last_line.ends_with(';') {
                        last_line.push(';');
                    }
                }
                i += 1;
                continue;
            }
            let s = if i == last {
                emit_expr_tail(e)
            } else {
                emit_expr(e)
            };
            // Unit-return tail: append `;` to discard the tail value.
            // Without this, a `bool`-returning tail like
            // `instance.save()` at the end of a `fn () -> ()` trips
            // E0308. Lit::Nil tail was already handled above; this
            // covers the non-Nil expression case (Send returning T,
            // Var/Ivar reads, etc.). The `()` block-value falls
            // through implicitly after the `;`.
            if i == last && !current_return_is_unit() {
                lines.push(s);
            } else {
                lines.push(format!("{s};"));
            }
            i += 1;
        }
        lines.join("\n")
    })
}

pub(super) fn emit_return(value: &Expr) -> String {
    let is_nil = matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
    // Constructor early returns produce `Self { fields }` — Ruby's
    // `return if cond` lowers to `Return { Nil }`, but a `pub fn new
    // (...) -> Self` body returning bare `()` wouldn't typecheck.
    if in_constructor() && is_nil {
        return format!("return {}", render_self_literal());
    }
    if is_nil {
        // `return nil` in a method declared `-> T?` (lowered as
        // `Option<T>`) must emit `return None`; bare `return` is E0069
        // outside `()` / Unit returns.
        if current_return_is_option() {
            "return None".to_string()
        } else {
            "return".to_string()
        }
    } else {
        // String-literal return in a String-returning function:
        // append `.to_string()`. Skip when str_color has already
        // stamped a STR_TO_OWNED / STR_BORROW bit on the value.
        let str_color_handled = super::has_str_coercion(value);
        let needs_to_string = !str_color_handled
            && matches!(
                &*value.node,
                ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
            )
            && matches!(
                current_return_ty().as_ref(),
                Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym)
            );
        // `return self` in a method declared `-> Base` — clone to
        // satisfy the owned return type.
        let needs_self_clone = matches!(&*value.node, ExprNode::SelfRef)
            && matches!(
                current_return_ty().as_ref(),
                Some(crate::ty::Ty::Class { .. })
            );
        // `return X` in an Option<T>-returning fn where X is typed T
        // (non-Option). Emit's job to insert the Some-wrap.
        let needs_some_wrap = current_return_is_option()
            && match value.ty.as_ref() {
                Some(t) if !super::util::is_option_ty(t) => true,
                _ => false,
            };
        // A `return X` always places X in return position, even when the
        // `return` statement is physically nested in a non-tail spot
        // (e.g. the guard `return @cache if @loaded` that sits as the
        // first stmt of a Seq). Set the return-tail flag so value-emit's
        // tail-aware coercions fire — notably the Ivar arm's `.clone()`
        // for a non-Copy field read, which otherwise moves out of `&self`
        // (E0507). Without this the eager-load guard `return
        // self.comments_cache` failed to compile (issue #27).
        if needs_to_string {
            format!("return {}.to_string()", super::with_return_tail(true, || emit_expr_tail(value)))
        } else if needs_self_clone {
            "return self.clone()".to_string()
        } else if needs_some_wrap {
            format!("return Some({})", super::with_return_tail(true, || emit_expr_tail(value)))
        } else {
            format!("return {}", super::with_return_tail(true, || emit_expr_tail(value)))
        }
    }
}

pub(super) fn emit_bool_op(
    op: &crate::expr::BoolOpKind,
    left: &Expr,
    right: &Expr,
) -> String {
    // Ruby `a && b` / `a || b` are truthy-on-non-nil-non-false, not
    // bool-typed. Rust's `||` / `&&` are bool-only — direct emit only
    // works when both operands are already Ty::Bool.
    //
    // For `Or` with a non-bool LHS, the idiomatic Ruby use is "default
    // value if LHS is nil/missing": `a || b` →
    //   - LHS Option<T>: `a.unwrap_or(b)`
    //   - LHS non-Option: `a` alone (Ruby's non-nil values are all
    //     truthy, so the RHS branch is unreachable)
    if matches!(op, crate::expr::BoolOpKind::Or) {
        let lhs_is_option = matches!(
            left.ty.as_ref(),
            Some(crate::ty::Ty::Union { variants })
                if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
        );
        let lhs_is_bool = matches!(left.ty.as_ref(), Some(crate::ty::Ty::Bool));
        if lhs_is_option {
            // `hash[k] || default` — the body-typer types `hash[k]` as
            // `Option<V>`, but rust2 emits Send `[]` as `hash[k]`
            // (panic-on-miss, returns &V). Detect and emit
            //   `recv.get(k).cloned().unwrap_or(default)`
            // directly — actually produces Option<V>.
            if let ExprNode::Send { recv: Some(r), method, args, .. } = &*left.node {
                if method.as_str() == "[]"
                    && args.len() == 1
                    && matches!(
                        r.ty.as_ref().map(peel_nil),
                        Some(crate::ty::Ty::Hash { .. })
                    )
                {
                    let key_s = emit_expr(&args[0]);
                    // Hash<_, Untyped> recv → `.get(k).cloned()`
                    // returns `Option<Value>`. Primitive default
                    // literals need `Value::from(...)` to type-unify.
                    let value_ty_untyped = matches!(
                        r.ty.as_ref().map(peel_nil),
                        Some(crate::ty::Ty::Hash { value, .. })
                            if matches!(value.as_ref(), crate::ty::Ty::Untyped)
                    );
                    // String default literal -> `.to_string()` (defer
                    // to str_color if already annotated).
                    let default_s = if value_ty_untyped {
                        coerce_to_value_default(right, emit_expr(right))
                    } else {
                        match &*right.node {
                            ExprNode::Lit { value: Literal::Str { .. } }
                                if !super::has_str_coercion(right) =>
                            {
                                format!("{}.to_string()", emit_expr(right))
                            }
                            _ => emit_expr(right),
                        }
                    };
                    // Thread-local module-singleton slot (ViewHelpers
                    // @slots) — borrow in place instead of cloning the
                    // map snapshot out of the slot first.
                    if let Some(get_s) = super::module_singleton_hash_get(r, &key_s) {
                        return format!("{get_s}.unwrap_or({default_s})");
                    }
                    let recv_s = emit_expr(r);
                    return format!(
                        "{recv_s}.get({key_s}).cloned().unwrap_or({default_s})"
                    );
                }
            }
            // `Option<Untyped>` (Value) `||` literal — the default
            // needs to be `Value`-shaped.
            let lhs_inner_untyped = matches!(
                left.ty.as_ref().map(peel_nil),
                Some(crate::ty::Ty::Untyped)
            );
            // `Option<Str>` (rust2 emits as `Option<String>`) `||`
            // literal-str — `unwrap_or` expects `String`, but Str
            // literal emits as `&'static str`. Force `.to_string()` on
            // the literal default so the type unifies.
            let lhs_inner_str = matches!(
                left.ty.as_ref().map(peel_nil),
                Some(crate::ty::Ty::Str | crate::ty::Ty::Sym)
            );
            let rhs_s = emit_expr(right);
            let default_s = if lhs_inner_untyped {
                coerce_to_value_default(right, rhs_s)
            } else if lhs_inner_str
                && matches!(
                    &*right.node,
                    ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                )
                && !super::has_str_coercion(right)
            {
                format!("{rhs_s}.to_string()")
            } else {
                rhs_s
            };
            return format!("{}.unwrap_or({})", emit_expr(left), default_s);
        }
        if !lhs_is_bool && left.ty.is_some() {
            // Statically non-nil — RHS unreachable in Ruby semantics. Drop.
            return emit_expr(left);
        }
    }
    match op {
        // `&&` binds tighter than `||`: an `Or` operand inside an `And`
        // must be parenthesized or precedence flips (`a && b || c`
        // parses as `(a && b) || c`, not `a && (b || c)`). This is the
        // ONLY boolean-operand case that needs parens — And-in-And,
        // And-in-Or, Or-in-Or, comparisons, and casts all bind at least
        // as tightly, so wrapping them would only draw Rust's
        // `unused_parens` lint.
        crate::expr::BoolOpKind::And => {
            format!("{} && {}", emit_and_operand(left), emit_and_operand(right))
        }
        crate::expr::BoolOpKind::Or => {
            format!("{} || {}", emit_expr(left), emit_expr(right))
        }
    }
}

/// Emit an `&&` operand, parenthesizing it when it would otherwise emit
/// as a bare `||` (which binds looser than `&&`). See `emit_bool_op`.
fn emit_and_operand(e: &Expr) -> String {
    let s = emit_expr(e);
    if emits_as_or_infix(e) { format!("({s})") } else { s }
}

/// True iff `e` is an `Or` that `emit_bool_op` renders as the infix
/// `l || r` form — NOT the `Option` `unwrap_or` rewrite or the
/// statically-non-nil RHS-drop (both emit as a primary and need no
/// wrap). MUST stay in sync with `emit_bool_op`'s `Or` branch above:
/// option LHS → unwrap_or (non-infix); non-bool *typed* LHS → RHS
/// dropped (non-infix); bool or untyped LHS → falls through to infix.
fn emits_as_or_infix(e: &Expr) -> bool {
    let ExprNode::BoolOp { op: crate::expr::BoolOpKind::Or, left, .. } = &*e.node else {
        return false;
    };
    let lhs_is_option = matches!(
        left.ty.as_ref(),
        Some(crate::ty::Ty::Union { variants })
            if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
    );
    let lhs_is_bool = matches!(left.ty.as_ref(), Some(crate::ty::Ty::Bool));
    !lhs_is_option && (lhs_is_bool || left.ty.is_none())
}

pub(super) fn emit_case(scrutinee: &Expr, arms: &[crate::expr::Arm]) -> String {
    // `case scrutinee; when Pat; body; …; end` → Rust `match`. Used by
    // the model lowerer's `synth_index_read` / `synth_index_write`
    // (get_index / set_index), which dispatch on a Symbol-typed `name`
    // param against per-column literal patterns.
    //
    // Wildcard arm: synthesized based on the enclosing return type —
    // `Value::Null` for `Value`-returning fns, `()` for unit-returning
    // fns. Without an `_` arm, the match isn't exhaustive over `&str`.
    let scrutinee_s = emit_expr(scrutinee);
    let return_ty = current_return_ty();
    let return_is_value = matches!(return_ty.as_ref(), Some(crate::ty::Ty::Untyped));
    let arm_strs: Vec<String> = arms
        .iter()
        .map(|arm| {
            let pat_s = emit_case_pattern(&arm.pattern);
            // Emit via `emit_expr_tail` so Ivar reads see
            // `IN_RETURN_TAIL=true` and add `.clone()` for non-Copy
            // fields. Without that, `Value::from(self.body)` below
            // would move out of `&self.body` (E0507).
            let body_s = emit_expr_tail(&arm.body);
            let body_wrapped = if return_is_value && !arm_body_already_value(&arm.body) {
                format!("serde_json::Value::from({body_s})")
            } else {
                body_s
            };
            format!("        {pat_s} => {{ {body_wrapped} }}")
        })
        .collect();
    let default_arm = if return_is_value {
        "serde_json::Value::Null".to_string()
    } else {
        "()".to_string()
    };
    format!(
        "match {scrutinee_s} {{\n{}\n        _ => {default_arm},\n    }}",
        arm_strs.join(",\n"),
    )
}

/// Detect a standalone Ruby guard-clause on a Var/param:
///   return X if name.nil?
/// (or `raise X if name.nil?`). The body-typer narrows `name` to
/// non-nil for subsequent statements, but in Rust source `name` is
/// still `Option<T>` from its parameter declaration / earlier let.
/// Emit
///   let Some(name) = name else { <then-branch> };
/// which rebinds `name` to the unwrapped value.
fn try_emit_param_guard_unwrap(guard: &Expr) -> Option<(String, String)> {
    use crate::ty::Ty;
    let ExprNode::If { cond, then_branch, else_branch } = &*guard.node else {
        return None;
    };
    let ExprNode::Send { recv: Some(cond_recv), method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "nil?" || !args.is_empty() {
        return None;
    }
    let ExprNode::Var { name: var_name, .. } = &*cond_recv.node else {
        return None;
    };
    let recv_is_option = matches!(
        cond_recv.ty.as_ref(),
        Some(Ty::Union { variants }) if variants.iter().any(|v| matches!(v, Ty::Nil))
    );
    if !recv_is_option {
        return None;
    }
    let then_diverges = matches!(then_branch.ty.as_ref(), Some(Ty::Bottom));
    let else_is_nil = matches!(
        &*else_branch.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    if !then_diverges || !else_is_nil {
        return None;
    }
    let diverge_s = emit_expr_tail(then_branch);
    let n = var_name.as_str().to_string();
    Some((
        n.clone(),
        format!("let Some({n}) = {n} else {{ {diverge_s} }}"),
    ))
}

/// Detect the Ruby idiom
///   x = OPT
///   return ... if x.nil?
/// (or `raise ... if x.nil?`). Emit as
///   let Some(x) = <opt> else { <then-branch> };
fn try_fuse_let_else(assign: &Expr, guard: &Expr) -> Option<(String, String)> {
    use crate::ty::Ty;
    let ExprNode::Assign { target, value } = &*assign.node else {
        return None;
    };
    let LValue::Var { name: assign_name, .. } = target else {
        return None;
    };
    let value_is_option = matches!(
        value.ty.as_ref(),
        Some(Ty::Union { variants }) if variants.iter().any(|v| matches!(v, Ty::Nil))
    );
    if !value_is_option {
        return None;
    }
    let ExprNode::If { cond, then_branch, else_branch } = &*guard.node else {
        return None;
    };
    let ExprNode::Send { recv: Some(cond_recv), method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "nil?" || !args.is_empty() {
        return None;
    }
    let ExprNode::Var { name: cond_name, .. } = &*cond_recv.node else {
        return None;
    };
    if cond_name != assign_name {
        return None;
    }
    let then_diverges = matches!(then_branch.ty.as_ref(), Some(Ty::Bottom));
    let else_is_nil = matches!(
        &*else_branch.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    if !then_diverges || !else_is_nil {
        return None;
    }
    let value_s = emit_expr(value);
    let diverge_s = emit_expr_tail(then_branch);
    let n = assign_name.as_str().to_string();
    Some((
        n.clone(),
        format!("let Some({n}) = {value_s} else {{ {diverge_s} }}"),
    ))
}

/// Wrap a literal/Var default with `serde_json::Value::from(...)` when
/// it's going to be passed to an `unwrap_or` on an
/// `Option<serde_json::Value>`. Skip the wrap when the expression
/// already produces a `Value`. Used by the BoolOp::Or peepholes.
fn coerce_to_value_default(default_expr: &Expr, raw: String) -> String {
    use crate::ty::Ty;
    let primitive = matches!(
        default_expr.ty.as_ref(),
        Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool)
    ) || matches!(
        &*default_expr.node,
        ExprNode::Lit {
            value: Literal::Str { .. }
                | Literal::Sym { .. }
                | Literal::Int { .. }
                | Literal::Float { .. }
                | Literal::Bool { .. }
        }
    );
    if primitive {
        format!("serde_json::Value::from({raw})")
    } else {
        raw
    }
}

/// True when the branch's tail expression — after walking through a
/// trailing `Seq` — is a Var read whose recorded `local_var_ty` is
/// already `Option<T>`. Used by the `if` tail-position Some-wrap to
/// avoid re-wrapping into `Option<Option<T>>`.
fn tail_produces_option(branch: &Expr) -> bool {
    let (tail_name, exprs) = match &*branch.node {
        ExprNode::Seq { exprs } => match exprs.last() {
            Some(last) => match &*last.node {
                ExprNode::Var { name, .. } => (Some(name.as_str().to_string()), exprs.as_slice()),
                _ => return false,
            },
            None => return false,
        },
        ExprNode::Var { name, .. } => (Some(name.as_str().to_string()), &[] as &[Expr]),
        _ => return false,
    };
    let Some(name) = tail_name else { return false };
    if !current_return_is_option() {
        return false;
    }
    if exprs.is_empty() {
        return true;
    }
    exprs.iter().any(|e| matches!(
        &*e.node,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { name: assign_name, .. },
            value,
        } if assign_name.as_str() == name
            && matches!(&*value.node, ExprNode::Lit { value: Literal::Nil })
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit::rust2::EmitCtx;
    use crate::emit::rust2::expr::with_emit_ctx;
    use crate::expr::{BoolOpKind, BoolOpSurface};
    use crate::ident::{Symbol, VarId};
    use crate::span::Span;
    use crate::ty::Ty;

    fn bool_var(name: &str) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: VarId(0), name: Symbol::from(name) },
        );
        e.ty = Some(Ty::Bool);
        e
    }

    fn bool_op(op: BoolOpKind, left: Expr, right: Expr) -> Expr {
        let mut e = Expr::new(
            Span::synthetic(),
            ExprNode::BoolOp { op, surface: BoolOpSurface::Symbol, left, right },
        );
        e.ty = Some(Ty::Bool);
        e
    }

    fn emit(e: &Expr) -> String {
        with_emit_ctx(EmitCtx::default(), || emit_expr(e))
    }

    #[test]
    fn or_operand_inside_and_is_parenthesized() {
        // `a && b && (c || d)` — the bug dropped the parens, flipping
        // precedence to `(a && b && c) || d`. `||` binds looser than
        // `&&`, so the `Or` operand must be wrapped.
        let inner_and = bool_op(BoolOpKind::And, bool_var("a"), bool_var("b"));
        let inner_or = bool_op(BoolOpKind::Or, bool_var("c"), bool_var("d"));
        let root = bool_op(BoolOpKind::And, inner_and, inner_or);
        assert_eq!(emit(&root), "a && b && (c || d)");
    }

    #[test]
    fn equal_or_tighter_precedence_operands_get_no_parens() {
        // These must stay paren-free or rustc's `unused_parens` lint
        // fires. `||` is the only operator looser than `&&`.
        let or_in_or = bool_op(
            BoolOpKind::Or,
            bool_op(BoolOpKind::Or, bool_var("a"), bool_var("b")),
            bool_var("c"),
        );
        assert_eq!(emit(&or_in_or), "a || b || c");

        let and_in_or = bool_op(
            BoolOpKind::Or,
            bool_op(BoolOpKind::And, bool_var("a"), bool_var("b")),
            bool_var("c"),
        );
        assert_eq!(emit(&and_in_or), "a && b || c");

        let and_in_and = bool_op(
            BoolOpKind::And,
            bool_op(BoolOpKind::And, bool_var("a"), bool_var("b")),
            bool_var("c"),
        );
        assert_eq!(emit(&and_in_and), "a && b && c");
    }
}
