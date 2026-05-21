//! Generic Go body/expression emission — used by the model method
//! emitter and by other modules that need a fallback for arbitrary
//! `Expr` rendering.
//!
//! Forked 2026-05-21 from `src/emit/go/expr.rs` so go2 can evolve
//! the walker independently (Phase 2+ type-aware emit, lowered-IR
//! coverage, transpiled-runtime call shapes) without dragging
//! legacy go regressions.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ty::Ty;

// Reused verbatim from legacy go until go2 needs its own dispatch.
use crate::emit::go::shared::go_method_name;

/// Context threaded through the walker. Carries everything that's
/// context-sensitive — i.e. whose Go emit depends on the enclosing
/// class/method shape, not just the local Expr subtree.
///
/// `SelfRef` is the first variant that needs it: in a class method
/// (`def self.foo`), `self.bar(x)` emits as the bare-fn call
/// `ClassName_bar(x)`; in an instance method it emits as `self.Bar(x)`
/// against the Go `(self *ClassName)` receiver. Without ctx, the
/// walker has no way to know which to pick.
#[derive(Debug, Clone)]
pub(super) struct EmitCtx {
    /// Enclosing class name, already sanitized to a Go identifier
    /// (e.g. `JsonBuilder`, `ActiveRecordBase`, not the raw
    /// `ActiveRecord::Base`). `None` when emitting a module-mode
    /// bag of bare functions or outside any class.
    pub class_name: Option<String>,
    /// True when emitting inside a class method (`def self.foo`).
    /// Class methods in Go are bare functions named `Class_method`;
    /// `SelfRef` inside them refers to the class itself, NOT to a
    /// receiver parameter. Instance methods set this `false`.
    pub in_class_method: bool,
}

impl EmitCtx {
    pub fn none() -> Self {
        Self {
            class_name: None,
            in_class_method: false,
        }
    }
}

pub(super) fn emit_expr(ctx: &EmitCtx, e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => name.to_string(),
        ExprNode::Send { recv, method, args, .. } => {
            emit_send(ctx, recv.as_ref(), method.as_str(), args)
        }
        // `self` reference. In an instance method, the Go body has
        // `(self *Class)` and `self` is a valid identifier — emit
        // verbatim. In a class method, there's no `self` parameter;
        // bare `SelfRef` would compile to a dangling identifier.
        // Emit the sanitized class name as a stand-in (it parses as
        // a Go reference to the type itself, surfacing the gap if
        // the surrounding context expects an instance). Without a
        // class name in ctx (module-mode), emit a TODO marker.
        ExprNode::SelfRef => self_ref_expr(ctx),
        // Ruby `x = value` — legacy go drops the lvalue and emits
        // only the rhs, which loses the binding. go2 needs the real
        // assignment so subsequent statements can refer to `x`.
        ExprNode::Assign { target, value } => emit_assign(ctx, target, value),
        // Ruby `return X` — emits as a Go return statement. In Ruby
        // this is technically an Expr (type `Never`), so it can
        // appear in value position; the body-position walker
        // (`emit_return_body`) intercepts most uses. The remaining
        // path is `return X if cond` lowered to `If { then: Return,
        // else: Nil }` inside a `Seq`; legacy `emit_expr` for If
        // walks branches via `emit_block_body`, so the Return ends
        // up emitted as a statement inside a Go block — valid.
        ExprNode::Return { value } => format!("return {}", emit_expr(ctx, value)),
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(|sub| emit_expr(ctx, sub))
            .collect::<Vec<_>>()
            .join("; "),
        ExprNode::If { cond, then_branch, else_branch } => {
            // `if recv.is_a?(Class)` → Go's type-assert init form
            // `if _, ok := recv.(GoTy); ok` when the class maps to
            // a Go assertion type. Otherwise plain cond emit.
            let (init, cond_s) = if let Some((init, ok)) = try_emit_is_a_init(ctx, cond) {
                (init, ok.to_string())
            } else {
                (String::new(), emit_expr(ctx, cond))
            };
            let then_s = emit_block_body(ctx, then_branch);
            // `return X if cond` lowers to If { else: Lit::Nil } —
            // emit without the else clause so the body parses as
            // valid Go (a bare `nil` statement is invalid).
            if is_nil_lit(else_branch) {
                format!("if {init}{cond_s} {{\n{then_s}\n}}")
            } else {
                let else_s = emit_block_body(ctx, else_branch);
                format!("if {init}{cond_s} {{\n{then_s}\n}} else {{\n{else_s}\n}}")
            }
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(ctx, k), emit_expr(ctx, v)))
                .collect();
            format!("map[string]interface{{}}{{{}}}", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_expr(ctx, e)).collect();
            format!("[]interface{{}}{{{}}}", parts.join(", "))
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(ctx, left), op_s, emit_expr(ctx, right))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut fmt = String::new();
            let mut args: Vec<String> = Vec::new();
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '%' {
                                fmt.push_str("%%");
                            } else {
                                fmt.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        fmt.push_str("%v");
                        args.push(emit_expr(ctx, expr));
                    }
                }
            }
            if args.is_empty() {
                format!("{fmt:?}")
            } else {
                format!("fmt.Sprintf({fmt:?}, {})", args.join(", "))
            }
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

pub(super) fn emit_send(
    ctx: &EmitCtx,
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_expr(ctx, a)).collect();

    if method == "[]" && recv.is_some() {
        let recv_s = emit_expr(ctx, recv.unwrap());
        // `str[start..end]` / `str[start..]` — Ruby Range becomes Go
        // slice syntax `str[start:end]`. Inclusive ranges add `+1` to
        // the end bound; exclusive ranges pass it through; open-ended
        // (no begin / no end) maps to Go's empty-side slice form.
        if args.len() == 1 {
            if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                let begin_s = begin
                    .as_ref()
                    .map(|e| emit_expr(ctx, e))
                    .unwrap_or_default();
                let end_s = match (end.as_ref(), *exclusive) {
                    (Some(e), true) => emit_expr(ctx, e),
                    (Some(e), false) => format!("{}+1", emit_expr(ctx, e)),
                    (None, _) => String::new(),
                };
                return format!("{recv_s}[{begin_s}:{end_s}]");
            }
        }
        // `str[start, length]` — Ruby's two-arg substring form. Map
        // to Go's `recv[start : start+length]`. Note: `start` is
        // emitted twice; safe for simple values (literal, var) but
        // re-evaluates side effects. Lowered runtime bodies don't
        // hit this pattern with side-effecting starts today; if
        // they do later, introduce a temp binding (needs statement
        // context, deferred).
        if args.len() == 2 {
            let start = &args_s[0];
            let length = &args_s[1];
            return format!("{recv_s}[{start}:{start}+{length}]");
        }
        return format!("{recv_s}[{}]", args_s.join(", "));
    }

    // Binary operators: Ruby parses `a == b`, `a + b`, etc. as
    // `Send { recv: a, method: "==", args: [b] }`. Emit them infix.
    if let (Some(r), Some(op)) = (recv, binary_op(method)) {
        if args.len() == 1 {
            return format!("{} {} {}", emit_expr(ctx, r), op, args_s[0]);
        }
    }

    // Ruby `.length` and `.size` on collection-like receivers → Go's
    // `len()` builtin. Maps identically across String, Array, Hash;
    // user-defined `length` methods on custom classes are rare in
    // the runtime/ruby/ source and not yet observed in practice.
    if (method == "length" || method == "size") && args.is_empty() {
        if let Some(r) = recv {
            return format!("len({})", emit_expr(ctx, r));
        }
    }

    // Ruby `recv.is_a?(SingletonClass)` for the three singleton
    // boolean-/nil-classes maps to direct equality, which is a plain
    // bool expression that fits anywhere `is_a?` appears (not just
    // at `if`-cond position). The mappable-class cases (Integer,
    // Float, String, ...) require Go's `_, ok := v.(T)` form, which
    // is statement-level and gets handled by the per-If/per-return
    // walker (`emit_expr::If` + `emit_return_at::If`).
    if method == "is_a?" && args.len() == 1 {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*args[0].node {
                if let Some(class) = path.last() {
                    let recv_s = emit_expr(ctx, r);
                    match class.as_str() {
                        "TrueClass" => return format!("{recv_s} == true"),
                        "FalseClass" => return format!("{recv_s} == false"),
                        "NilClass" => return format!("{recv_s} == nil"),
                        _ => {}
                    }
                }
            }
        }
    }

    // Ruby `.nil?` predicate → Go nil comparison. The receiver's
    // Ty (when known) drives the result:
    //
    //   non-nilable primitives (Str, Sym, Int, Float, Bool) — Ruby
    //     `nil?` is statically false; emit `false`. Avoids invalid
    //     Go like `string == nil`.
    //   Ty::Nil — statically true.
    //   anything else (Untyped, Union, Class, unknown) — emit
    //     `recv == nil`, which works against interface{}, pointer,
    //     map, slice, and channel receivers.
    //
    // Without analyzer-filled `Expr.ty` the receiver appears
    // typeless and falls to `== nil`, which is the safe default for
    // the `interface{}` shape go2 emits for unknown types.
    if method == "nil?" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            return match r.ty.as_ref() {
                Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool) => {
                    "false".to_string()
                }
                Some(Ty::Nil) => "true".to_string(),
                _ => format!("{recv_s} == nil"),
            };
        }
    }

    // Ruby `.to_s` → Go string conversion. String-typed receiver is
    // a no-op; numeric receivers use the matching Sprintf verb;
    // everything else (including untyped `interface{}`) falls back
    // to `%v` — which delegates to `fmt.Stringer` if implemented
    // and is a reasonable default otherwise. Without analyzer-filled
    // `Expr.ty` the receiver appears typeless and we land on `%v`.
    if method == "to_s" && args.is_empty() {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            return match r.ty.as_ref() {
                Some(Ty::Str | Ty::Sym) => recv_s,
                Some(Ty::Int) => format!("fmt.Sprintf(\"%d\", {recv_s})"),
                Some(Ty::Float) => format!("fmt.Sprintf(\"%g\", {recv_s})"),
                _ => format!("fmt.Sprintf(\"%v\", {recv_s})"),
            };
        }
    }

    // SelfRef receiver in a class method context — rewrite to the
    // bare-fn call `ClassName_method(args)` because class methods
    // emit as bare functions, not as methods on a struct. Use the
    // bare-fn name shape (lowercase + `?`/`!`/`=` sanitized, no
    // pascalize) to match the function-definition shape in
    // `library::emit_method`.
    if let (Some(r), Some(class_name)) = (recv, ctx.class_name.as_deref()) {
        if ctx.in_class_method && matches!(&*r.node, ExprNode::SelfRef) {
            let m = super::library::sanitize_method_name(method);
            return format!("{class_name}_{m}({})", args_s.join(", "));
        }
    }

    // Ruby→Go method-name mapping for string operations that have no
    // 1:1 in Go's stdlib (`strip` is `strings.TrimSpace(…)`, not
    // `.Strip()`). Only kicks in for instance dispatch on Str-typed
    // receivers; class calls and unknown types pass through.
    if let Some(r) = recv {
        if args.is_empty() && matches!(r.ty, Some(Ty::Str)) {
            if let Some(wrapped) = map_go_str_method(method, &emit_expr(ctx, r)) {
                return wrapped;
            }
        }
    }

    let go_m = go2_method_ident(method);
    match recv {
        None => {
            if args_s.is_empty() {
                go_m
            } else {
                format!("{}({})", go_m, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(ctx, r);
            // Struct field access vs method call: 0-arg Sends on a
            // non-Class receiver whose method isn't a known AR/stdlib
            // call render without parens (`p.Title`, not `p.Title()`).
            let is_class_call = matches!(&*r.node, ExprNode::Const { .. });
            if !is_class_call && args_s.is_empty() && !is_known_go_method(method) {
                return format!("{recv_s}.{go_m}");
            }
            format!("{}.{}({})", recv_s, go_m, args_s.join(", "))
        }
    }
}

/// `SelfRef` in value position (not as a Send receiver). Class
/// methods don't have a `self` binding in Go, so emit the class
/// name itself as the most useful identifier-shape stand-in. Code
/// downstream of this fallback might still produce nonsense, but at
/// least the file parses and the gap is locally visible.
fn self_ref_expr(ctx: &EmitCtx) -> String {
    if ctx.in_class_method {
        match ctx.class_name.as_deref() {
            Some(n) => n.to_string(),
            None => "/* TODO: SelfRef without class context */".to_string(),
        }
    } else {
        // Instance method — the emitted Go has `(self *Class)` and
        // `self` is the receiver param name.
        "self".to_string()
    }
}

/// Ruby method names with `?` / `!` suffixes don't translate to Go
/// identifiers. Rewrite to `_p` / `_bang` form before passing
/// through the standard pascalize path: `nil?` → `nil_p` → `NilP`,
/// `save!` → `save_bang` → `SaveBang`. Semantic translation
/// (`nil?` → `== nil`, `is_a?(C)` → type assertion) is a separate
/// widening; this only handles the identifier-shape side so emit
/// produces parseable Go.
fn go2_method_ident(ruby_name: &str) -> String {
    let normalized = ruby_name.replace('?', "_p").replace('!', "_bang");
    go_method_name(&normalized)
}

fn is_nil_lit(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

/// Recognize `recv.is_a?(Const)` and return `(recv, last-segment of
/// Const path)` so the caller can splice in Go's type-assert
/// if-init form. Returns `None` for any other shape, including
/// `is_a?(SomeRuntimeValue)` (rare; emit falls back to the generic
/// `IsAP` send and the gap stays visible).
fn is_a_predicate<'a>(e: &'a Expr) -> Option<(&'a Expr, &'a str)> {
    let ExprNode::Send { recv, method, args, .. } = &*e.node else {
        return None;
    };
    if method.as_str() != "is_a?" || args.len() != 1 {
        return None;
    }
    let r = recv.as_ref()?;
    let ExprNode::Const { path } = &*args[0].node else {
        return None;
    };
    Some((r, path.last()?.as_str()))
}

/// Map a Ruby class name to the Go type used by `v.(T)` assertion.
/// Returns `None` for classes whose Go counterpart needs more
/// context (Hash → `map[K]V`, Array → `[]E`, user-defined classes)
/// — those fall through to the bare-call emit so the gap stays
/// visible.
fn ruby_class_to_go_assert_ty(class: &str) -> Option<&'static str> {
    Some(match class {
        "Integer" => "int64",
        "Float" => "float64",
        "String" => "string",
        "Symbol" => "string",
        _ => return None,
    })
}

/// Build `_, ok := recv.(GoTy)` init + `ok` cond pair for an
/// `is_a?` predicate that has a mapped Go assertion type. Returns
/// `None` if either the shape or the class isn't supported, so
/// callers can fall through to the unchanged path.
fn try_emit_is_a_init(ctx: &EmitCtx, cond: &Expr) -> Option<(String, &'static str)> {
    let (recv, class) = is_a_predicate(cond)?;
    let go_ty = ruby_class_to_go_assert_ty(class)?;
    let recv_s = emit_expr(ctx, recv);
    Some((format!("_, ok := {recv_s}.({go_ty}); "), "ok"))
}

/// Emit a Ruby `x = value` as a Go assignment. Uses `:=` for the
/// common case (fresh local binding) and falls back to `=` when the
/// lvalue isn't a plain local. Targets covered:
///
/// - `LValue::Var` → `name := value`
/// - `LValue::Ivar` → `self.name = value` (instance receiver)
/// - other → emit just the value with a `/* TODO */` marker so the
///   gap is loudly visible in the v2/ output
fn emit_assign(ctx: &EmitCtx, target: &crate::expr::LValue, value: &Expr) -> String {
    use crate::expr::LValue;
    let v = emit_expr(ctx, value);
    match target {
        LValue::Var { name, .. } => format!("{name} := {v}"),
        LValue::Ivar { name } => format!("self.{name} = {v}"),
        _ => format!("/* TODO: emit Assign target shape */ _ = {v}"),
    }
}

/// Ruby method names that map to Go binary operators when called
/// with a receiver and one argument. `nil` semantics for `==` differ
/// (Go nil interface vs typed nil) but that's a downstream concern —
/// at this level we just rewrite the call shape.
fn binary_op(method: &str) -> Option<&'static str> {
    Some(match method {
        "==" => "==",
        "!=" => "!=",
        "<" => "<",
        "<=" => "<=",
        ">" => ">",
        ">=" => ">=",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "%" => "%",
        _ => return None,
    })
}

/// AR/stdlib method names that should emit with parens on a model
/// struct receiver. Everything else on a non-Class receiver with no
/// args is treated as a field read. Grows alongside the runtime.
fn is_known_go_method(name: &str) -> bool {
    matches!(
        name,
        "save" | "save!" | "destroy" | "destroy!" | "update" | "update!"
            | "delete" | "touch" | "reload"
            | "validate" | "attributes" | "errors"
    )
}

/// Map Ruby String methods onto Go expressions that compile. `strip`
/// in Ruby is `strings.TrimSpace(s)` in Go — no method form exists.
/// Returns `Some(emit_text)` for a handled method. Unhandled methods
/// fall through to the default `.Method()` emit which may or may not
/// compile depending on the target receiver's actual methods.
fn map_go_str_method(method: &str, recv_text: &str) -> Option<String> {
    match method {
        "strip" => Some(format!("strings.TrimSpace({recv_text})")),
        "upcase" => Some(format!("strings.ToUpper({recv_text})")),
        "downcase" => Some(format!("strings.ToLower({recv_text})")),
        _ => None,
    }
}

pub(super) fn emit_block_body(ctx: &EmitCtx, e: &Expr) -> String {
    let raw = match &*e.node {
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(|sub| emit_expr(ctx, sub))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => emit_expr(ctx, e),
    };
    raw.lines().map(|l| format!("\t{l}")).collect::<Vec<_>>().join("\n")
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "nil".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, flags } => {
            format!("regexp.MustCompile({:?})", format!("(?{flags}){pattern}"))
        }
    }
}

/// Emit `expr` at body (return) position — Ruby's last-expression
/// semantics mapped to Go's explicit `return`. Recurses into `If`
/// and `Seq` so the return lands at the value-producing leaf. All
/// other variants emit as `return <value_expression>`.
///
/// Output is indented one tab in (caller wraps in `func ... { ... }`).
pub(super) fn emit_return_body(ctx: &EmitCtx, e: &Expr) -> String {
    let mut out = String::new();
    emit_return_at(ctx, e, &mut out, 1);
    out
}

fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push('\t');
    }
}

fn emit_return_at(ctx: &EmitCtx, e: &Expr, out: &mut String, depth: usize) {
    match &*e.node {
        ExprNode::If { cond, then_branch, else_branch } => {
            let (init, cond_s) = if let Some((init, ok)) = try_emit_is_a_init(ctx, cond) {
                (init, ok.to_string())
            } else {
                (String::new(), emit_expr(ctx, cond))
            };
            indent(out, depth);
            out.push_str(&format!("if {init}{cond_s} {{\n"));
            emit_return_at(ctx, then_branch, out, depth + 1);
            // Skip the else clause when it's an implicit nil — the
            // `return X if cond` shape doesn't want a `nil` else.
            if !is_nil_lit(else_branch) {
                indent(out, depth);
                out.push_str("} else {\n");
                emit_return_at(ctx, else_branch, out, depth + 1);
            }
            indent(out, depth);
            out.push_str("}\n");
        }
        ExprNode::Seq { exprs } => {
            // All but the last are statements (effects-only); last is
            // the return-position expression.
            for (i, sub) in exprs.iter().enumerate() {
                if i + 1 == exprs.len() {
                    emit_return_at(ctx, sub, out, depth);
                } else {
                    indent(out, depth);
                    out.push_str(&emit_expr(ctx, sub));
                    out.push('\n');
                }
            }
        }
        ExprNode::Return { value } => {
            // Already a return; don't double up to `return return X`.
            let v = emit_expr(ctx, value);
            indent(out, depth);
            out.push_str(&format!("return {v}\n"));
        }
        _ => {
            let v = emit_expr(ctx, e);
            indent(out, depth);
            out.push_str(&format!("return {v}\n"));
        }
    }
}
