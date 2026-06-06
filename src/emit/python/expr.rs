//! Generic Python expression / body / statement renderer.
//!
//! Reused by the model-method emitter and as a fallback for the
//! controller-action walker.

use std::cell::{Cell, RefCell};

use crate::expr::{Expr, ExprNode, LValue, Literal, OpAssignOp};
use crate::ty::Ty;

thread_local! {
    /// How `ExprNode::SelfRef` renders in the body currently being
    /// emitted: `"self"` in an instance method, `"cls"` in a classmethod.
    /// A lowering injects explicit `SelfRef` receivers for implicit-self
    /// calls (`table_name` → `self.table_name`), so a class method full
    /// of those needs `cls`, not `self`. Defaults to `"self"` for any
    /// caller that doesn't set it (app-code instance methods).
    static SELF_REF: Cell<&'static str> = const { Cell::new("self") };

    /// The Python name of the method whose body is currently being
    /// emitted (already mapped: `initialize` → `__init__`). An
    /// `ExprNode::Super` dispatches to the *parent's* same-named method,
    /// which Python spells `super().<method>(args)` — so the arm needs
    /// the enclosing method's name. `None` outside a library-class method
    /// body (app-code emit paths don't set it; they don't carry `Super`).
    static SUPER_METHOD: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run `f` with `ExprNode::SelfRef` rendering as `name` (`"self"` or
/// `"cls"`), restoring the previous setting after. The library emitter
/// wraps each method body with the receiver-appropriate name.
pub(super) fn with_self_ref<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    let prev = SELF_REF.with(|c| c.replace(name));
    let r = f();
    SELF_REF.with(|c| c.set(prev));
    r
}

/// Run `f` with `ExprNode::Super` dispatching to `method` (the enclosing
/// method's already-mapped Python name), restoring the previous setting
/// after. The library emitter wraps each method body so a `super(args)`
/// inside it renders as `super().<method>(args)`.
pub(super) fn with_super_method<R>(method: &str, f: impl FnOnce() -> R) -> R {
    let prev = SUPER_METHOD.with(|c| c.borrow_mut().replace(method.to_string()));
    let r = f();
    SUPER_METHOD.with(|c| *c.borrow_mut() = prev);
    r
}

thread_local! {
    /// True while emitting a library *class*-method body, where a bare
    /// (receiverless) non-kernel call is an implicit-self call that the
    /// shared self-injection lowering left unresolved (it only covers
    /// methods defined on the class, so inherited/builtin `name`/`new`
    /// stay barewords). When set, `emit_send` prefixes such calls with
    /// the `SELF_REF` receiver — mirroring the TS emitter's `rewrite`.
    /// Off for module functions (inflector/json_builder) and app-code
    /// fallback paths, where bare calls are genuine free functions.
    static INJECT_SELF_SENDS: Cell<bool> = const { Cell::new(false) };
}

/// Run `f` with implicit-self injection for bare non-kernel calls on
/// (see `INJECT_SELF_SENDS`), restoring the previous setting after.
pub(super) fn with_self_sends<R>(on: bool, f: impl FnOnce() -> R) -> R {
    let prev = INJECT_SELF_SENDS.with(|c| c.replace(on));
    let r = f();
    INJECT_SELF_SENDS.with(|c| c.set(prev));
    r
}

/// Ruby Kernel/module methods that stay receiverless in a class-method
/// body (they are not implicit-self calls). Mirrors the TS emitter's
/// `is_kernel_call`.
fn is_kernel_call(method: &str) -> bool {
    matches!(
        method,
        "raise" | "puts" | "print" | "p" | "pp"
            | "require" | "require_relative" | "load" | "autoload"
    )
}

/// True if `body` contains an `ExprNode::Yield` anywhere — the library
/// emitter uses this to decide whether to inject a `_block` parameter
/// into the method signature. Ported from the TS emitter's
/// `body_contains_yield`; covers every node that can nest a yield.
pub(super) fn body_contains_yield(body: &Expr) -> bool {
    match &*body.node {
        ExprNode::Yield { .. } => true,
        ExprNode::Seq { exprs } => exprs.iter().any(body_contains_yield),
        ExprNode::If { cond, then_branch, else_branch } => {
            body_contains_yield(cond)
                || body_contains_yield(then_branch)
                || body_contains_yield(else_branch)
        }
        ExprNode::Case { scrutinee, arms } => {
            body_contains_yield(scrutinee)
                || arms.iter().any(|a| {
                    a.guard.as_ref().is_some_and(body_contains_yield)
                        || body_contains_yield(&a.body)
                })
        }
        ExprNode::Send { recv, args, block, .. } => {
            recv.as_ref().is_some_and(body_contains_yield)
                || args.iter().any(body_contains_yield)
                || block.as_ref().is_some_and(body_contains_yield)
        }
        ExprNode::Apply { fun, args, block } => {
            body_contains_yield(fun)
                || args.iter().any(body_contains_yield)
                || block.as_ref().is_some_and(body_contains_yield)
        }
        ExprNode::Lambda { body: lb, .. } => body_contains_yield(lb),
        ExprNode::Assign { target, value } => {
            (match target {
                LValue::Attr { recv, .. } | LValue::Index { recv, .. } => {
                    body_contains_yield(recv)
                }
                _ => false,
            }) || body_contains_yield(value)
        }
        ExprNode::OpAssign { target, value, .. } => {
            (match target {
                LValue::Attr { recv, .. } | LValue::Index { recv, .. } => {
                    body_contains_yield(recv)
                }
                _ => false,
            }) || body_contains_yield(value)
        }
        ExprNode::Return { value } => body_contains_yield(value),
        ExprNode::Raise { value } => body_contains_yield(value),
        ExprNode::Next { value } | ExprNode::Break { value } => {
            value.as_ref().is_some_and(body_contains_yield)
        }
        ExprNode::Splat { value } => body_contains_yield(value),
        ExprNode::Super { args } => args
            .as_ref()
            .is_some_and(|v| v.iter().any(body_contains_yield)),
        ExprNode::BoolOp { left, right, .. } => {
            body_contains_yield(left) || body_contains_yield(right)
        }
        ExprNode::While { cond, body: wb, .. } => {
            body_contains_yield(cond) || body_contains_yield(wb)
        }
        ExprNode::RescueModifier { expr, fallback } => {
            body_contains_yield(expr) || body_contains_yield(fallback)
        }
        ExprNode::BeginRescue { body: inner, rescues, else_branch, ensure, .. } => {
            body_contains_yield(inner)
                || rescues.iter().any(|r| body_contains_yield(&r.body))
                || else_branch.as_ref().is_some_and(body_contains_yield)
                || ensure.as_ref().is_some_and(body_contains_yield)
        }
        _ => false,
    }
}

// Bodies + expressions -------------------------------------------------

pub(super) fn emit_body(body: &Expr, return_ty: &Ty) -> String {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, value } => {
            if is_void {
                emit_expr(value)
            } else {
                format!("return {}", emit_expr(value))
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 && e.leading_blank_line {
                    lines.push(String::new());
                }
                lines.push(emit_stmt(e, i == exprs.len() - 1, is_void));
            }
            lines.join("\n")
        }
        // A whole-body loop, explicit return, or case routes through the
        // statement emitter (all degrade in expression position).
        ExprNode::While { .. } | ExprNode::Return { .. } | ExprNode::Case { .. } => {
            emit_stmt(body, true, is_void)
        }
        _ => {
            if let Some(s) = try_emit_raise_stmt(body) {
                s
            } else if is_void {
                emit_expr(body)
            } else {
                format!("return {}", emit_expr(body))
            }
        }
    }
}

/// Build the Python exception value for a Ruby `raise`'s arguments:
/// `raise C, msg` → `C(msg)`; `raise "msg"` → `RuntimeError("msg")`;
/// `raise e` / `raise C.new(...)` → the value as-is. Empty for a bare
/// re-raise (`raise` with no args).
fn raise_exception_expr(args: &[Expr]) -> String {
    match args {
        [] => String::new(),
        [one] => match &*one.node {
            ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. } => {
                format!("RuntimeError({})", emit_expr(one))
            }
            _ => emit_expr(one),
        },
        [klass, msg, ..] => format!("{}({})", emit_expr(klass), emit_expr(msg)),
    }
}

/// If `e` is a Ruby `raise …` (a no-receiver `raise` Send), render it as
/// a Python `raise` *statement*. Ruby's `raise` is an expression, so it
/// reaches a method's tail where `emit_body`/`emit_stmt` would otherwise
/// wrap it in `return` — but a `raise` statement must stand alone.
fn try_emit_raise_stmt(e: &Expr) -> Option<String> {
    if let ExprNode::Send { recv: None, method, args, .. } = &*e.node {
        if method.as_str() == "raise" {
            return Some(match raise_exception_expr(args).as_str() {
                "" => "raise".to_string(),
                exc => format!("raise {exc}"),
            });
        }
    }
    None
}

pub(super) fn emit_stmt(e: &Expr, is_last: bool, void_return: bool) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("self.{} = {}", name, emit_expr(value))
        }
        // Compound assignment. Python's arithmetic/bitwise compound ops
        // are spelled identically to Ruby's (`+=`, `**=`, `<<=`, …), so
        // they pass through `as_ruby`. The short-circuit forms have no
        // Python compound operator: `x ||= y` → `x = x or y`.
        ExprNode::OpAssign { target, op, value } => {
            let lhs = lvalue_str(target);
            let rhs = emit_expr(value);
            match op {
                OpAssignOp::OrOr => format!("{lhs} = {lhs} or {rhs}"),
                OpAssignOp::AndAnd => format!("{lhs} = {lhs} and {rhs}"),
                _ => format!("{lhs} {} {rhs}", op.as_ruby()),
            }
        }
        // A nested `Seq` in statement position (e.g. the else-path of a
        // guard-clause body: `return X if g; <stmt; stmt; …>`). Emit each
        // element as a statement, threading is_last/void to the tail.
        // Without this it falls to `emit_expr`, which `;`-joins the seq —
        // dropping assignment LHSs and degrading any while/case inside.
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .enumerate()
            .map(|(i, s)| emit_stmt(s, is_last && i == exprs.len() - 1, void_return))
            .collect::<Vec<_>>()
            .join("\n"),
        // Explicit `return X` (Ruby `return foo`, incl. guard-clause
        // tails). Python has native return — no wrapping needed.
        ExprNode::Return { value } => format!("return {}", emit_expr(value)),
        // Guard return: `return X if cond` → `if cond: return X`. A
        // ternary would put the return in expression position (illegal).
        ExprNode::If { cond, then_branch, else_branch }
            if is_nil_or_empty(else_branch)
                && matches!(&*then_branch.node, ExprNode::Return { .. }) =>
        {
            let ExprNode::Return { value } = &*then_branch.node else { unreachable!() };
            format!("if {}: return {}", emit_expr(cond), emit_expr(value))
        }
        // Inverse guard: `return X unless cond` → `if not (cond): return
        // X` (the Return sits in the else branch, then is empty).
        ExprNode::If { cond, then_branch, else_branch }
            if is_nil_or_empty(then_branch)
                && matches!(&*else_branch.node, ExprNode::Return { .. }) =>
        {
            let ExprNode::Return { value } = &*else_branch.node else { unreachable!() };
            format!("if not ({}): return {}", emit_expr(cond), emit_expr(value))
        }
        // Postfix-`if` with no else, off the value-returning tail: emit a
        // native guard rather than a ternary, which would drop an
        // assignment LHS or other statement side effect.
        ExprNode::If { cond, then_branch, else_branch }
            if is_nil_or_empty(else_branch) && (!is_last || void_return) =>
        {
            match &*then_branch.node {
                ExprNode::Seq { .. } => {
                    format!("if {}:\n{}", emit_expr(cond), emit_block_body(then_branch, void_return))
                }
                _ => format!("if {}: {}", emit_expr(cond), emit_stmt(then_branch, false, true)),
            }
        }
        // General `if / elsif / else` in statement position (a non-empty
        // else, so the guard arms above didn't match). Emit native
        // `if/elif/else:` blocks so a `return`/assignment in any branch
        // stays a statement — a ternary (`emit_expr`'s `If` arm) would
        // force it into expression position, which Python forbids. Only
        // off the value-returning tail; a value-position `If` stays a
        // ternary.
        ExprNode::If { cond, then_branch, else_branch } if !is_last || void_return => {
            emit_if_stmt(cond, then_branch, else_branch, void_return)
        }
        // Iteration with a block in statement position: `recv.each do |k,
        // v| BODY end`. Python has no block-passing, so emit a native
        // `for` loop. A Hash receiver iterates `(k, v)` pairs via
        // `.items()`; an Array iterates elements directly. Unknown
        // receiver types degrade honestly rather than silently dropping
        // the block.
        ExprNode::Send { recv: Some(r), method, args, block: Some(block), .. }
            if method.as_str() == "each"
                && args.is_empty()
                && matches!(&*block.node, ExprNode::Lambda { .. }) =>
        {
            let ExprNode::Lambda { params, body, .. } = &*block.node else { unreachable!() };
            let pnames: Vec<String> = params.iter().map(|s| s.to_string()).collect();
            emit_for_each(r, &pnames, body).unwrap_or_else(|| {
                crate::emit::diagnostics::report_unsupported("python", "each-block", "")
            })
        }
        // `case scrutinee; when X then …; else …; end` → an `if/elif/else`
        // chain comparing the scrutinee against each `when` literal (the
        // per-column `[]`/`[]=` indexer the model lowering emits). Arm
        // bodies inherit this statement's `is_last`/`void` so a value-
        // returning case (`__getitem__`) `return`s from each branch.
        ExprNode::Case { scrutinee, arms } => emit_case_stmt(scrutinee, arms, is_last, void_return),
        // `while/until cond; body; end` → native loop. `until` negates
        // the condition (Python has no `until`).
        ExprNode::While { cond, body, until_form } => {
            let c = emit_expr(cond);
            let c = if *until_form { format!("not ({c})") } else { c };
            format!("while {c}:\n{}", emit_block_body(body, true))
        }
        _ => {
            if let Some(s) = try_emit_raise_stmt(e) {
                s
            } else if is_last && !void_return {
                format!("return {}", emit_expr(e))
            } else {
                emit_expr(e)
            }
        }
    }
}

/// Render Ruby `recv[...]` as a Python subscript. A single Range arg
/// becomes slice syntax; the two-arg `recv[start, length]` form becomes
/// `recv[start:start+length]`; otherwise a plain `recv[idx]`.
fn emit_index(recv_s: &str, args: &[Expr]) -> String {
    if let [idx] = args {
        if let ExprNode::Range { begin, end, exclusive } = &*idx.node {
            return format!("{recv_s}[{}]", range_slice(begin, end, *exclusive));
        }
    }
    if let [start, len] = args {
        let s = emit_expr(start);
        return format!("{recv_s}[{s}:{s}+{}]", emit_expr(len));
    }
    let parts: Vec<String> = args.iter().map(emit_expr).collect();
    format!("{recv_s}[{}]", parts.join(", "))
}

/// Render a Ruby range used as a slice index. Inclusive `a..b` maps to
/// Python's exclusive `a:b+1` (literal-folded, so `..-1` → `:` — to end
/// — and `..-2` → `:-1`); exclusive `a...b` → `a:b`; open ends omit the
/// bound (`a..` → `a:`).
fn range_slice(begin: &Option<Expr>, end: &Option<Expr>, exclusive: bool) -> String {
    let start = begin.as_ref().map(|e| emit_expr(e)).unwrap_or_default();
    let stop = match end {
        None => String::new(),
        Some(e) if exclusive => emit_expr(e),
        Some(e) => match &*e.node {
            ExprNode::Lit { value: Literal::Int { value: -1 } } => String::new(),
            ExprNode::Lit { value: Literal::Int { value } } => (value + 1).to_string(),
            _ => format!("{}+1", emit_expr(e)),
        },
    };
    format!("{start}:{stop}")
}

/// Render an assignment target as its Python left-hand side. Mirrors the
/// `Assign` arms in `emit_stmt` for the in-place compound-assign forms.
fn lvalue_str(t: &LValue) -> String {
    match t {
        LValue::Var { name, .. } => name.to_string(),
        LValue::Ivar { name } => format!("self.{name}"),
        LValue::Attr { recv, name } => format!("{}.{name}", emit_expr(recv)),
        LValue::Index { recv, index } => format!("{}[{}]", emit_expr(recv), emit_expr(index)),
        LValue::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
    }
}

/// True for an `If`'s absent else branch — Ruby's `x if cond` lowers to
/// `If { else_branch = nil }` (or an empty `Seq`).
fn is_nil_or_empty(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Lit { value: Literal::Nil } => true,
        ExprNode::Seq { exprs } => exprs.is_empty(),
        _ => false,
    }
}

/// Emit a general `if / elif / else` chain as native Python statements.
/// The `else` branch is rendered as `elif` when it's itself an `If` (an
/// `elsif` chain lowers to a nested `If` in the else slot) and as `else:`
/// otherwise; an empty else is omitted. Recurses for the `elif` case by
/// rewriting the child's leading `if ` to `elif `.
fn emit_if_stmt(cond: &Expr, then_branch: &Expr, else_branch: &Expr, void: bool) -> String {
    let mut out = format!("if {}:\n{}", emit_expr(cond), block_or_pass(then_branch, void));
    match &*else_branch.node {
        // Empty else (`x if c` / explicit nil tail) — nothing to emit.
        _ if is_nil_or_empty(else_branch) => {}
        // `elsif`: the else slot is another `If`. Recurse and splice the
        // child's `if …` onto an `el` to form `elif …`.
        ExprNode::If { cond: c2, then_branch: t2, else_branch: e2 } => {
            out.push('\n');
            out.push_str(&format!("el{}", emit_if_stmt(c2, t2, e2, void)));
        }
        // Plain `else` block.
        _ => {
            out.push('\n');
            out.push_str(&format!("else:\n{}", block_or_pass(else_branch, void)));
        }
    }
    out
}

/// Emit a `recv.each do |params| body end` call as a native Python `for`
/// loop. A 2-param block over a Hash iterates `(k, v)` via `.items()`; a
/// 1-param block over an Array iterates elements directly. Returns `None`
/// for shapes outside that (so the caller can degrade rather than emit a
/// wrong loop) — the block body would otherwise be silently dropped.
fn emit_for_each(recv: &Expr, params: &[String], body: &Expr) -> Option<String> {
    let recv_s = emit_expr(recv);
    let (vars, iter) = match (params.len(), recv.ty.as_ref()) {
        (2, Some(Ty::Hash { .. })) => {
            (format!("{}, {}", params[0], params[1]), format!("{recv_s}.items()"))
        }
        (1, Some(Ty::Array { .. })) => (params[0].clone(), recv_s),
        _ => return None,
    };
    Some(format!("for {vars} in {iter}:\n{}", block_or_pass(body, true)))
}

/// Emit a `case scrutinee; when LIT then …; else …; end` as a native
/// `if/elif/else` chain (`scrutinee == LIT`). Only literal `when`
/// patterns with no guard, plus a wildcard `else`, are handled — the
/// model lowering's per-column indexer is exactly this shape; anything
/// else degrades. Arm bodies are emitted with the case's `is_last`/`void`
/// so a value-returning case `return`s from each branch.
fn emit_case_stmt(scrutinee: &Expr, arms: &[crate::expr::Arm], is_last: bool, void: bool) -> String {
    use crate::expr::Pattern;
    let scr = emit_expr(scrutinee);
    let mut branches: Vec<String> = Vec::new();
    let mut else_body: Option<&Expr> = None;
    for arm in arms {
        if arm.guard.is_some() {
            return crate::emit::diagnostics::report_unsupported("python", "Case", "");
        }
        match &arm.pattern {
            Pattern::Wildcard => else_body = Some(&arm.body),
            Pattern::Lit { value } => {
                let kw = if branches.is_empty() { "if" } else { "elif" };
                branches.push(format!(
                    "{kw} {scr} == {}:\n{}",
                    emit_literal(value),
                    stmt_block(&arm.body, is_last, void)
                ));
            }
            _ => return crate::emit::diagnostics::report_unsupported("python", "Case", ""),
        }
    }
    if let Some(eb) = else_body {
        branches.push(format!("else:\n{}", stmt_block(eb, is_last, void)));
    }
    branches.join("\n")
}

/// Emit `e` as a 4-space-indented statement block, threading `is_last`/
/// `void` to the final statement (so a value-position branch `return`s).
/// A `Seq` becomes one statement per line; an empty body is `pass`.
fn stmt_block(e: &Expr, is_last: bool, void: bool) -> String {
    let inner = match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => exprs
            .iter()
            .enumerate()
            .map(|(i, s)| emit_stmt(s, is_last && i == exprs.len() - 1, void))
            .collect::<Vec<_>>()
            .join("\n"),
        ExprNode::Seq { .. } => "pass".to_string(),
        _ => emit_stmt(e, is_last, void),
    };
    super::shared::indent_py(&inner)
}

/// `emit_block_body`, but an empty body becomes `pass` so the compound
/// statement isn't a syntax error.
fn block_or_pass(e: &Expr, void: bool) -> String {
    let b = emit_block_body(e, void);
    if b.trim().is_empty() {
        "    pass".to_string()
    } else {
        b
    }
}

/// Emit `e` as the 4-space-indented body of a compound statement
/// (`while`/`if`). A `Seq` becomes one statement per line; anything else
/// is a single statement. Body statements are never the method's return
/// value, so `is_last` is false throughout.
fn emit_block_body(e: &Expr, void: bool) -> String {
    let inner = match &*e.node {
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(|s| emit_stmt(s, false, void))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_stmt(e, false, void),
    };
    super::shared::indent_py(&inner)
}

pub(super) fn emit_expr(e: &Expr) -> String {
    // Analyze may have annotated this expression as a user error
    // (e.g., Incompatible `+`). If so, emit the target raise-
    // equivalent instead of the normal rendering — matches Ruby's
    // behavior of raising at runtime.
    if let Some(kind) = &e.diagnostic {
        return crate::emit::diagnostics::StubStyle::PythonThrow
            .render(&crate::diagnostic::Diagnostic::stub_text(kind));
    }
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("self.{name}"),
        ExprNode::SelfRef => SELF_REF.with(|c| c.get()).to_string(),
        // `recv.map { |x| EXPR }` / `.collect` → Python list comprehension
        // `[EXPR for x in recv]`. Expression-position iteration (the
        // statement-position `.each` case lives in `emit_stmt`); without
        // this the block is silently dropped, leaving a broken `.map()`.
        ExprNode::Send { recv: Some(recv), method, block: Some(block), .. }
            if matches!(method.as_str(), "map" | "collect")
                && matches!(&*block.node, ExprNode::Lambda { .. }) =>
        {
            let ExprNode::Lambda { params, body, .. } = &*block.node else { unreachable!() };
            match params.as_slice() {
                [p] => format!("[{} for {} in {}]", emit_expr(body), p, emit_expr(recv)),
                _ => crate::emit::diagnostics::report_unsupported("python", "map-block", ""),
            }
        }
        ExprNode::Send { recv, method, args, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // Python ternary `a if cond else b`. `emit_expr` is always
            // called in an expression position (non-expression If flow
            // lives in the controller/view emitters with their own
            // statement-form handlers).
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_s = emit_expr(else_branch);
            format!("{then_s} if {cond_s} else {else_s}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "or",
                BoolOpKind::And => "and",
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    // Symbol keys become string keys in Python dicts
                    // (since Python has no atom type). Rails-style
                    // `{foo: 1}` → `{"foo": 1}`.
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{:?}: {}", value.as_str(), emit_expr(v))
                    } else {
                        format!("{}: {}", emit_expr(k), emit_expr(v))
                    }
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            // Python f-string — `f"text {expr} more"`. Reserved chars
            // inside the `{}` are rare for the Ruby expressions we
            // ingest, so no escape bookkeeping yet; if a fixture
            // triggers one we add it then.
            use crate::expr::InterpPart;
            let mut out = String::from("f\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '{' => out.push_str("{{"),
                                '}' => out.push_str("}}"),
                                '\n' => out.push_str("\\n"),
                                other => out.push(other),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push('{');
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        ExprNode::Cast { value, .. } => emit_expr(value),
        // `super(args)` → `super().<method>(args)`, where `<method>` is the
        // enclosing method's Python name (set by the library emitter via
        // `with_super_method`). Bare `super` (no parens — Ruby forwards the
        // method's own args implicitly) can't be reconstructed here without
        // the param list, so it stays a degrade until a degrade-free target
        // needs it.
        ExprNode::Super { args: Some(args) } => {
            let method = SUPER_METHOD
                .with(|c| c.borrow().clone())
                .unwrap_or_else(|| "__init__".to_string());
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            format!("super().{}({})", method, args_s.join(", "))
        }
        // `yield a, b` invokes the enclosing method's implicit block. The
        // library emitter injects a `_block` parameter into any method
        // whose body uses yield (see `body_contains_yield`); here we just
        // call it. Single-underscore name (not `__block`) to dodge
        // Python's `__name` class-private name mangling.
        ExprNode::Yield { args } => {
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            format!("_block({})", args_s.join(", "))
        }
        other => crate::emit::diagnostics::report_unsupported("python", other.kind_str(), ""),
    }
}

pub(super) fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    // Index / slice. Handle before computing `args_s`: a Range arg
    // (`x[a..b]`) must be destructured into slice syntax, and the eager
    // `args_s` below would emit the Range node — firing a degrade — as a
    // side effect even though the result would be discarded here.
    if method == "[]" {
        if let Some(r) = recv {
            return emit_index(&emit_expr(r), args);
        }
    }
    // Logical not. Ruby's `!x` rides the Send channel as `x.!()`; two
    // surface forms reach here — `Send { recv: Some(x), args: [] }` and
    // view_to_library's `Send { recv: None, args: [x] }`. Python spells
    // it `not`. `!x.nil?` collapses to the idiomatic `x is not None` (the
    // negation of the `nil?` → `is None` rewrite below); everything else
    // is `not (inner)` — parens are always safe since `not` binds looser
    // than comparison but tighter than `and`/`or`.
    if method == "!" {
        let inner = match (recv, args) {
            (Some(r), []) => Some(r),
            (None, [a]) => Some(a),
            _ => None,
        };
        if let Some(inner) = inner {
            if let ExprNode::Send { recv: Some(r), method: m, args: a, .. } = &*inner.node {
                if m.as_str() == "nil?" && a.is_empty() {
                    return format!("{} is not None", emit_expr(r));
                }
            }
            return format!("not ({})", emit_expr(inner));
        }
    }
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Ruby `X.new(args)` / implicit-self `new(args)` construct an
    // instance; Python calls the class directly. `Flash.new` → `Flash()`,
    // `new(attrs)` (implicit self in a class method) → `cls(attrs)`. A
    // receiverless `new` only rewrites inside a class-method body; bare
    // `new` elsewhere falls through to the generic call.
    if method == "new" {
        if let Some(r) = recv {
            return format!("{}({})", emit_expr(r), args_s.join(", "));
        }
        if INJECT_SELF_SENDS.with(|c| c.get()) {
            return format!("{}({})", SELF_REF.with(|c| c.get()), args_s.join(", "));
        }
    }
    // Ruby reflection `x.class` → Python `type(x)`. `class` is a Python
    // keyword, so it can never surface as a `.class` attribute or call.
    if method == "class" && args.is_empty() {
        if let Some(r) = recv {
            return format!("type({})", emit_expr(r));
        }
    }
    // Ruby's binary operators ride the Send channel (`a == b` is
    // `a.==(b)`). Python needs infix; emit as `recv op arg` for the
    // ones whose syntax matches 1:1. Equality against Nil gets the
    // idiomatic `is None` / `is not None` form when the body-typer
    // flagged one side as Ty::Nil.
    if let (Some(r), [arg]) = (recv, args) {
        if method == "==" || method == "!=" {
            use crate::emit::shared::eq::{classify_eq, EqCase};
            if let EqCase::NilCheck { subject } = classify_eq(r, arg) {
                let keyword = if method == "==" { "is" } else { "is not" };
                return format!("{} {keyword} None", emit_expr(subject));
            }
        }
        // `+` dispatch: Python's native `+` handles all of the
        // supported cases (numeric, string, list). The dispatch's
        // only behavior change is rejecting Incompatible pairs
        // (Int+Str, Hash+Hash, …) that Ruby would raise on.
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            if matches!(classify_add(r, arg), AddCase::Incompatible) {
                // Emit a runtime raise in Python, matching Ruby's
                // behavior (Ruby would raise TypeError at this line).
                // `raise` is a statement, so use a generator `.throw`
                // trick to keep the form expression-valued.
                return r#"(_ for _ in ()).throw(TypeError("roundhouse: + with incompatible operand types"))"#.to_string();
            }
        }
        // `-` dispatch: Python supports numeric `-` natively; list
        // difference needs a comprehension. Incompatible refuses.
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            match classify_sub(r, arg) {
                SubCase::ArrayDifference { .. } => {
                    return format!(
                        "[x for x in {} if x not in {}]",
                        emit_expr(r),
                        emit_expr(arg)
                    );
                }
                SubCase::Incompatible => {
                    return r#"(_ for _ in ()).throw(TypeError("roundhouse: - with incompatible operand types"))"#.to_string();
                }
                _ => {}
            }
        }
        // `*` dispatch: Python's native `*` handles numeric, string
        // repeat, and list repeat. Only array-join needs `.join(...)`;
        // Incompatible pairs refuse.
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            match classify_mul(r, arg) {
                MulCase::ArrayJoin { .. } => {
                    // `sep.join(str(x) for x in arr)` — Python's idiom.
                    return format!(
                        "{}.join(str(x) for x in {})",
                        emit_expr(arg),
                        emit_expr(r)
                    );
                }
                MulCase::Incompatible => {
                    return r#"(_ for _ in ()).throw(TypeError("roundhouse: * with incompatible operand types"))"#.to_string();
                }
                _ => {}
            }
        }
        // `/` and `**` dispatch: Python has both natively. Ruby's Int/Int
        // is integer division (towards -infinity); Python's `/` is true
        // division, and `//` is floor. For now emit `/` unconditionally;
        // refine if an Int/Int case forces the floor-div distinction.
        if method == "/" || method == "**" {
            use crate::emit::shared::div_pow::{classify_div_pow, DivPowCase};
            if matches!(classify_div_pow(r, arg), DivPowCase::Incompatible) {
                return format!(
                    r#"(_ for _ in ()).throw(TypeError("roundhouse: `{method}` with incompatible operand types"))"#
                );
            }
        }
        // `%` dispatch: Python's native `%` covers numeric and string
        // format directly (same printf-style as Ruby). Only refuse
        // Incompatible pairs.
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            if matches!(classify_modulo(r, arg), ModuloCase::Incompatible) {
                return r#"(_ for _ in ()).throw(TypeError("roundhouse: % with incompatible operand types"))"#.to_string();
            }
        }
        if is_py_binop(method) {
            return format!("{} {} {}", emit_expr(r), method, emit_expr(arg));
        }
    }
    match recv {
        None => {
            // Expression-position `raise` — statement contexts intercept
            // this earlier via `try_emit_raise_stmt`. `raise` is a Python
            // statement, so stay expression-valued with the generator
            // `.throw` trick (same device as the incompatible-`+` path).
            if method == "raise" {
                let exc = raise_exception_expr(args);
                return if exc.is_empty() {
                    "(_ for _ in ()).throw(RuntimeError())".to_string()
                } else {
                    format!("(_ for _ in ()).throw({exc})")
                };
            }
            // Implicit-self call in a class-method body that the shared
            // self-injection lowering left a bareword (inherited/builtin
            // `name`, etc.). Inject the `SELF_REF` receiver, mirroring the
            // TS emit rewrite. `name` is Ruby's `Module#name` (the class
            // name) → Python's `cls.__name__`.
            if INJECT_SELF_SENDS.with(|c| c.get()) && !is_kernel_call(method) {
                let recv = SELF_REF.with(|c| c.get());
                if method == "name" && args_s.is_empty() {
                    return format!("{recv}.__name__");
                }
                let m = super::shared::py_method_name(method);
                return if args_s.is_empty() {
                    format!("{recv}.{m}()")
                } else {
                    format!("{recv}.{m}({})", args_s.join(", "))
                };
            }
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            // Ruby type predicates map to Python builtins on the
            // receiver, not to a name-legalized method call.
            if method == "nil?" && args.is_empty() {
                return format!("{recv_s} is None");
            }
            if matches!(method, "is_a?" | "kind_of?" | "instance_of?") {
                if let [arg] = args {
                    return ruby_isinstance(&recv_s, &emit_expr(arg));
                }
            }
            // Ruby stdlib methods → Python builtins, gated on the
            // receiver's inferred type so user methods of the same name
            // (Flash#length/#to_h, a model #to_s) aren't shadowed.
            if let Some(mapped) = map_builtin_method(&recv_s, method, r.ty.as_ref(), &args_s) {
                return mapped;
            }
            // Every other `?`/`!` (and `[]`/`[]=`) name is legalized the
            // same way at the definition site, so calls and defs align.
            let m = super::shared::py_method_name(method);
            if args_s.is_empty() {
                // Bare `recv.method` (no parens) is how Python accesses
                // attributes. For method calls with no args we emit
                // `recv.method()` — matches Python idiom and avoids
                // confusing a 0-arity call with an attribute read.
                format!("{recv_s}.{m}()")
            } else {
                format!("{recv_s}.{m}({})", args_s.join(", "))
            }
        }
    }
}

/// Strip a single-non-nil `Union<T, Nil>` down to `T`; leave any other
/// type unchanged. Lets type-gated builtin maps fire on a value whose
/// static type is nullable (array index, pre-assignment ivar, …).
fn strip_nullable(ty: Option<&Ty>) -> Option<Ty> {
    match ty {
        Some(Ty::Union { variants }) => {
            let non_nil: Vec<&Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect();
            match non_nil.as_slice() {
                [single] => Some((*single).clone()),
                _ => ty.cloned(),
            }
        }
        _ => ty.cloned(),
    }
}

/// Map a Ruby stdlib method call to its Python equivalent, gated on the
/// receiver's inferred type. Returns `None` to fall through to the
/// generic call emit (preserving user methods that share a name with a
/// Ruby builtin, e.g. `Flash#length`, `Session#to_h`).
fn map_builtin_method(recv: &str, method: &str, ty: Option<&Ty>, args_s: &[String]) -> Option<String> {
    // A receiver's inferred type is often nullable: `Array#[]` is
    // `Union<elem, Nil>` (an index can be out of bounds), an ivar's
    // flow-sensitive type carries `Nil` until first assignment, etc. The
    // builtin maps below should still fire on the inner type (e.g.
    // `pattern_parts[i].start_with?(...)` → `.startswith()`), so strip a
    // single-non-nil `Union<T, Nil>` down to `T` first. Mirrors the TS
    // emitter's `strip_nullable`.
    let stripped = strip_nullable(ty);
    let ty = stripped.as_ref();
    let no_args = args_s.is_empty();
    let one_arg = args_s.len() == 1;
    let is_class = matches!(ty, Some(Ty::Class { .. }));
    let is_str = matches!(ty, Some(Ty::Str | Ty::Sym));
    let is_array = matches!(ty, Some(Ty::Array { .. }));
    let is_seq = matches!(ty, Some(Ty::Str | Ty::Sym | Ty::Array { .. } | Ty::Hash { .. }));
    Some(match method {
        // Universal Ruby conversions → Python builtins (which accept any
        // value). Skipped on known user classes, which may define their
        // own to_s/to_i — there `str(x)` would call __str__, not the
        // Ruby-named method.
        "to_s" if no_args && !is_class => format!("str({recv})"),
        "to_i" if no_args && !is_class => format!("int({recv})"),
        "to_f" if no_args && !is_class => format!("float({recv})"),
        // Collection/string ops → Python builtins, gated to the builtin
        // type so user methods of the same name aren't shadowed.
        "length" | "size" if no_args && is_seq => format!("len({recv})"),
        // `empty?` on a builtin collection/string → `len(x) == 0`. Gated
        // to seq types so Flash#empty?/Session#empty? (user methods) keep
        // their own definitions.
        "empty?" if no_args && is_seq => format!("len({recv}) == 0"),
        "to_h" if no_args && matches!(ty, Some(Ty::Hash { .. })) => format!("dict({recv})"),
        "to_a" if no_args && matches!(ty, Some(Ty::Array { .. })) => format!("list({recv})"),
        // Ruby's single-element `Array#push(x)` is Python's `list.append`.
        // (Multi-arg push falls through — it doesn't occur in framework
        // code and Python's append takes one element.)
        "push" if one_arg && is_array => format!("{recv}.append({})", args_s[0]),
        "upcase" if no_args && is_str => format!("{recv}.upper()"),
        "downcase" if no_args && is_str => format!("{recv}.lower()"),
        "start_with?" if one_arg && is_str => format!("{recv}.startswith({})", args_s[0]),
        "end_with?" if one_arg && is_str => format!("{recv}.endswith({})", args_s[0]),
        _ => return None,
    })
}

/// Map a Ruby `is_a?(Class)` check to a Python membership test. Builtin
/// classes become `isinstance` against the Python type (or `is True/False/
/// None` for the singleton classes); user classes fall through to
/// `isinstance` on the last name segment.
fn ruby_isinstance(recv: &str, cls: &str) -> String {
    let base = cls.rsplit("::").next().unwrap_or(cls);
    let base = base.rsplit('.').next().unwrap_or(base);
    match base {
        "Integer" => format!("isinstance({recv}, int)"),
        "Float" => format!("isinstance({recv}, float)"),
        "Numeric" => format!("isinstance({recv}, (int, float))"),
        "String" | "Symbol" => format!("isinstance({recv}, str)"),
        "Array" => format!("isinstance({recv}, list)"),
        "Hash" => format!("isinstance({recv}, dict)"),
        "TrueClass" => format!("{recv} is True"),
        "FalseClass" => format!("{recv} is False"),
        "NilClass" => format!("{recv} is None"),
        other => format!("isinstance({recv}, {other})"),
    }
}

fn is_py_binop(method: &str) -> bool {
    matches!(
        method,
        "==" | "!="
            | "<"
            | "<="
            | ">"
            | ">="
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "<<"
            | ">>"
            | "|"
            | "&"
            | "^"
    )
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "None".to_string(),
        Literal::Bool { value } => {
            if *value { "True".to_string() } else { "False".to_string() }
        }
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => super::shared::py_string_literal(value),
        // Symbols have no direct Python equivalent; emit as string
        // literals. Enum detection would refine this into a typed
        // Enum subclass later.
        Literal::Sym { value } => super::shared::py_string_literal(value.as_str()),
        Literal::Regex { pattern, flags } => {
            let flag_expr = python_regex_flag_expr(flags);
            if flag_expr.is_empty() {
                format!("re.compile({pattern:?})")
            } else {
                format!("re.compile({pattern:?}, {flag_expr})")
            }
        }
    }
}

fn python_regex_flag_expr(flags: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if flags.contains('i') { parts.push("re.IGNORECASE"); }
    if flags.contains('m') { parts.push("re.MULTILINE"); }
    if flags.contains('x') { parts.push("re.VERBOSE"); }
    parts.join(" | ")
}
