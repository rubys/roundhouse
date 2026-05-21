//! Generic Go body/expression emission — used by the model method
//! emitter and by other modules that need a fallback for arbitrary
//! `Expr` rendering.
//!
//! Forked 2026-05-21 from `src/emit/go/expr.rs` so go2 can evolve
//! the walker independently (Phase 2+ type-aware emit, lowered-IR
//! coverage, transpiled-runtime call shapes) without dragging
//! legacy go regressions.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::expr::{Expr, ExprNode, Literal};
use crate::ty::Ty;

// Reused verbatim from legacy go until go2 needs its own dispatch.
use crate::emit::go::shared::{go_field_name, go_method_name};

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
    /// Var name → replacement identifier. Used by the
    /// `is_a?(Class)` if-init rewrite: when `if _, ok := v.(string)`
    /// is rewritten to `if s, ok := v.(string)`, the then_branch
    /// emits with `v` → `s` so the asserted typed value is what
    /// nested call sites consume. Scoped to the child ctx; never
    /// applied to the else branch or the outer scope.
    pub var_renames: HashMap<String, String>,
    /// Names already declared in the enclosing method body —
    /// function params at entry, plus any `Var` lvalues assigned
    /// previously in source order. `emit_assign` picks `:=` on the
    /// first assignment to a name and `=` on subsequent ones,
    /// matching Ruby's flat-method-scope semantics (an inner Ruby
    /// `x = 1` reassigns the outer `x`; Go's `:=` would shadow,
    /// silently losing the write). Shared across child ctxs via
    /// `Rc<RefCell>` so the scope flattens — `with_rename` clones
    /// the Rc, not the contents.
    pub declared: Rc<RefCell<HashSet<String>>>,
    /// True when the enclosing method's signature returns `void`
    /// (RBS `() -> void` lowers to `Ty::Fn { ret: Ty::Nil, ... }`).
    /// emit_return_at suppresses the implicit `return X` wrap when
    /// set — Ruby methods have an implicit Lit::Nil tail that
    /// shouldn't emit as `return nil` against a Go void return type.
    pub void_method: bool,
    /// True when emitting a method on a Ruby module-singleton class
    /// (e.g. `module ActiveRecord; class << self; attr_accessor :adapter;
    /// end; end`). All methods of such a class have Class receiver, and
    /// every `@ivar` reference targets a per-slot package var
    /// (`<ClassName>_<ivar>_slot`) instead of an instance field or a
    /// bare module-level var. The library emit detects the shape and
    /// flips this flag; without it, Ivar reads/writes would either
    /// hit `self.Field` (no instance exists) or a bare `<ivar>` that
    /// collides across modules.
    pub in_module_singleton: bool,
}

impl EmitCtx {
    pub fn none() -> Self {
        Self {
            class_name: None,
            in_class_method: false,
            var_renames: HashMap::new(),
            declared: Rc::new(RefCell::new(HashSet::new())),
            void_method: false,
            in_module_singleton: false,
        }
    }

    /// Seed the declared-names set with method parameter names so
    /// `Assign` to a param emits as `=` not `:=`. Idempotent.
    pub fn declare_param(&self, name: &str) {
        self.declared.borrow_mut().insert(name.to_string());
    }

    /// Build a child ctx with one extra Var rename pushed in. Used
    /// by the `If` handlers to expose the asserted typed value to
    /// the then_branch. Shares the declared-names set via Rc clone.
    pub fn with_rename(&self, from: String, to: String) -> Self {
        let mut child = self.clone();
        child.var_renames.insert(from, to);
        child
    }
}

pub(super) fn emit_expr(ctx: &EmitCtx, e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => {
            // Substitute via ctx.var_renames if present — used by
            // the `is_a?` if-init rewrite to expose the asserted
            // typed value inside the then_branch.
            ctx.var_renames
                .get(name.as_str())
                .cloned()
                .unwrap_or_else(|| name.to_string())
        }
        // `@field` in instance method bodies maps to `self.Field`
        // (the Go struct field synthesized by attr_reader/writer in
        // library.rs). In class methods (`def self.foo`), `@field`
        // refers to module-singleton state — emit as a bare
        // lowercase name that resolves to the package-level `var`
        // generated by `format_module_ivar`. When the class is a
        // module-singleton (`is_module=true` + every method Class
        // receiver), the package var is namespaced by class name
        // (`<Class>_<ivar>_slot`) to avoid collisions across modules
        // that happen to share an ivar name (`@adapter` on two
        // unrelated modules would otherwise both bind to `var adapter`).
        ExprNode::Ivar { name } => {
            if ctx.in_module_singleton {
                if let Some(class) = ctx.class_name.as_deref() {
                    format!("{class}_{}_slot", name.as_str())
                } else {
                    name.as_str().to_string()
                }
            } else if ctx.in_class_method {
                name.as_str().to_string()
            } else {
                format!("self.{}", go_field_name(name.as_str()))
            }
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_send(ctx, recv.as_ref(), method.as_str(), args, block.as_ref())
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
        ExprNode::While { cond, body, until_form } => {
            // Ruby `while cond ... end` → Go `for cond { ... }`.
            // `until cond` (the `until_form` flag) negates the cond:
            // `for !(cond) { ... }`. Ruby `while` evaluates to nil;
            // Go's `for` is a statement. emit_return_at's `_ =>`
            // fallback wraps the while value as `return …`, which
            // is invalid — body-position while loops will need a
            // dedicated emit_return_at::While arm (deferred until
            // a body actually puts a while at tail position).
            let cond_s = emit_expr(ctx, cond);
            let cond_text = if *until_form {
                format!("!({cond_s})")
            } else {
                cond_s
            };
            let body_s = emit_block_body(ctx, body);
            format!("for {cond_text} {{\n{body_s}\n}}")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // `unless cond ... end` lowers to `If { then: Lit::Nil,
            // else: ... }`. Invert before emit so we don't produce
            // an invalid bare-nil then-block.
            if is_nil_lit(then_branch) && !is_nil_lit(else_branch) {
                let cond_s = emit_expr(ctx, cond);
                let else_s = emit_block_body(ctx, else_branch);
                return format!("if !({cond_s}) {{\n{else_s}\n}}");
            }
            // `if recv.is_a?(Class)` → Go's type-assert init form
            // `if asserted, ok := recv.(GoTy); ok`. The then_branch
            // gets a child ctx that renames the recv's Var to the
            // asserted ident, so nested uses see the typed value.
            let (init, cond_s, then_ctx) = match try_emit_is_a_init(ctx, cond) {
                Some(IsAInit { init, cond, recv_name, asserted_ident }) => {
                    let child = match recv_name {
                        Some(n) => ctx.with_rename(n, asserted_ident.to_string()),
                        None => ctx.clone(),
                    };
                    (init, cond.to_string(), child)
                }
                None => (String::new(), emit_expr(ctx, cond), ctx.clone()),
            };
            let then_s = emit_block_body(&then_ctx, then_branch);
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
            // Infer concrete element types when every key/value is a
            // string literal (`{ "k" => "v", ... }`) — emits
            // `map[string]string` instead of `map[string]interface{}`.
            // Matters for `gsub(regex, hash)` lookups whose return
            // must satisfy a `string` return type.
            let all_str_kv = entries.iter().all(|(k, v)| {
                matches!(&*k.node, ExprNode::Lit { value: Literal::Str { .. } })
                    && matches!(&*v.node, ExprNode::Lit { value: Literal::Str { .. } })
            });
            let (k_ty, v_ty) = if all_str_kv {
                ("string", "string")
            } else {
                ("string", "interface{}")
            };
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(ctx, k), emit_expr(ctx, v)))
                .collect();
            format!("map[{k_ty}]{v_ty}{{{}}}", parts.join(", "))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|e| emit_expr(ctx, e)).collect();
            format!("[]interface{{}}{{{}}}", parts.join(", "))
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            // Ruby `||` returns the first truthy operand; Go's `||`
            // requires bool operands. For non-bool operand types
            // (Ty::Str typically — `slots[k] || ""` / `form_class ||
            // "button_to"`), use `cmp.Or` (Go 1.22+) which returns
            // the first non-zero value. Detect by either side being
            // known-string. And-on-strings is rare; leave the
            // legacy `&&` for now.
            if matches!(op, BoolOpKind::Or) {
                let stringy =
                    matches!(left.ty, Some(Ty::Str)) || matches!(right.ty, Some(Ty::Str));
                if stringy {
                    return format!(
                        "cmp.Or({}, {})",
                        emit_expr(ctx, left),
                        emit_expr(ctx, right)
                    );
                }
            }
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
                go_str_literal(&fmt)
            } else {
                format!("fmt.Sprintf({}, {})", go_str_literal(&fmt), args.join(", "))
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
    block: Option<&Expr>,
) -> String {
    let args_s: Vec<String> = args.iter().map(|a| emit_expr(ctx, a)).collect();

    // `recv.each { |x| body }` (1-param) and `recv.each { |k, v| body }`
    // (2-param) → Go `for ... range` loop wrapped in an IIFE that
    // returns the receiver (Ruby `each` semantics). The IIFE wrap
    // makes the statement-shaped loop fit anywhere an expression
    // goes — assignment value, Seq middle, method tail. Return type
    // `interface{}` keeps the wrap total without needing receiver-
    // type inference at this emit site; callers that discard the
    // value (`Seq` middle, `each` in void-method tail) silently
    // drop it, matching Go's "discarded return is fine" semantics.
    if method == "each" && args.is_empty() {
        if let (Some(recv_e), Some(block_e)) = (recv, block) {
            if let ExprNode::Lambda { params, body, .. } = &*block_e.node {
                let recv_s = emit_expr(ctx, recv_e);
                // Loop vars: 1 param → array iter (drop the index
                // with `_`); 2 params → hash iter (key + value);
                // 0 params (rare — `arr.each { puts "hi" }`) → both
                // sides bound to `_`. >2 params is unmappable; emit
                // a TODO so the gap is loudly visible.
                let body_ctx = ctx.clone();
                let range_vars = match params.len() {
                    0 => "_, _".to_string(),
                    1 => {
                        let name = params[0].as_str();
                        body_ctx.declare_param(name);
                        format!("_, {name}")
                    }
                    2 => {
                        let k = params[0].as_str();
                        let v = params[1].as_str();
                        body_ctx.declare_param(k);
                        body_ctx.declare_param(v);
                        format!("{k}, {v}")
                    }
                    _ => {
                        return format!(
                            "/* TODO: each block with {} params */",
                            params.len(),
                        );
                    }
                };
                let body_s = emit_block_body(&body_ctx, body);
                return format!(
                    "func() interface{{}} {{\n\
                     \tfor {range_vars} := range {recv_s} {{\n\
                     {body_s}\n\
                     \t}}\n\
                     \treturn {recv_s}\n\
                     }}()",
                );
            }
        }
    }

    // Ruby `recv[k] = v` is sugar for `recv.[]=(k, v)` in the IR.
    // Emit as a Go index-assign statement. Note: in Go this is a
    // statement, not an expression — emitting at expression position
    // (e.g. inside `return ...`) produces invalid Go. In practice
    // these Sends only appear at statement position in lowered
    // bodies, so the emit is safe today.
    if method == "[]=" && args.len() == 2 && recv.is_some() {
        let recv_s = emit_expr(ctx, recv.unwrap());
        return format!("{recv_s}[{}] = {}", args_s[0], args_s[1]);
    }

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

    // Ruby `.empty?` predicate on String/Array/Hash → Go
    // `len(recv) == 0`. Same scope as `length` — collection-shaped.
    if method == "empty?" && args.is_empty() {
        if let Some(r) = recv {
            return format!("len({}) == 0", emit_expr(ctx, r));
        }
    }

    // Ruby `arr << x` (Array push) parses as a Send `<<` with one
    // arg. Go: `arr = append(arr, x)`. Statement, not expression —
    // the lowered bodies use this at statement position. For Hash
    // shovel (`hash << pair`) the semantics differ; if a non-Array
    // recv hits this, the resulting Go `append(map, ...)` errors
    // at compile, surfacing the gap.
    if method == "<<" && args.len() == 1 {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            return format!("{recv_s} = append({recv_s}, {})", args_s[0]);
        }
    }

    // Ruby `.freeze` / `.to_h` — both pass through the receiver
    // unchanged in Go (no immutability marker; `.to_h` is a no-op
    // on Ruby Hash and would convert NamedTuple → Hash under
    // Crystal, neither of which Go needs).
    if (method == "freeze" || method == "to_h") && args.is_empty() {
        if let Some(r) = recv {
            return emit_expr(ctx, r);
        }
    }

    // Ruby `!x` parses as `Send { recv: x, method: "!", args: [] }`.
    // Emit as Go's unary not. Parenthesize the operand for safety
    // against operator-precedence surprises.
    if method == "!" && args.is_empty() {
        if let Some(r) = recv {
            return format!("!({})", emit_expr(ctx, r));
        }
    }

    // Ruby `h.delete(k)` → Go's `delete(h, k)` builtin. Statement
    // form. For non-Hash receivers (Array#delete-by-value, etc.)
    // this emit is wrong; the runtime/ruby/ surface only uses the
    // Hash form today.
    if method == "delete" && args.len() == 1 {
        if let Some(r) = recv {
            return format!("delete({}, {})", emit_expr(ctx, r), args_s[0]);
        }
    }

    // Ruby `h.fetch(k, default)` → Go `cmp.Or(h[k], default)`.
    // Subtle semantic gap: Ruby fetch returns default ONLY if the
    // key is missing; cmp.Or returns default when h[k] is the zero
    // value (which is "" / 0 / nil for missing keys but also for
    // explicitly-stored zero values). Acceptable for the
    // runtime/ruby/ surface today; revisit if call sites
    // distinguish missing vs zero.
    if method == "fetch" && args.len() == 2 {
        if let Some(r) = recv {
            return format!(
                "cmp.Or({}[{}], {})",
                emit_expr(ctx, r),
                args_s[0],
                args_s[1]
            );
        }
    }

    // Ruby `s.tr(from, to)` (1-char from/to) → Go's
    // `strings.ReplaceAll(s, from, to)`. Multi-char tr (`tr("ab",
    // "12")` mapping a→1, b→2) needs per-char translation —
    // approximate with ReplaceAll for now; gap surfaces only on
    // multi-char tr which the runtime doesn't use.
    if method == "tr" && args.len() == 2 {
        if let Some(r) = recv {
            return format!(
                "strings.ReplaceAll({}, {}, {})",
                emit_expr(ctx, r),
                args_s[0],
                args_s[1]
            );
        }
    }

    // Ruby `arr.join(sep)` → Go `strings.Join(arr, sep)`. Requires
    // arr to be `[]string`; `[]interface{}` arr would compile error
    // surfacing the type-inference gap (Array literals currently
    // default to `[]interface{}` regardless of element types).
    if method == "join" && args.len() == 1 {
        if let Some(r) = recv {
            return format!(
                "strings.Join({}, {})",
                emit_expr(ctx, r),
                args_s[0]
            );
        }
    }

    // Ruby `recv.gsub(pattern_regex, hash_or_string)` →
    // `pattern.ReplaceAll{String,StringFunc}(recv, ...)` against
    // Go's regexp package. Hash replacement uses
    // `ReplaceAllStringFunc` with a closure that looks up each
    // match in the hash; string replacement uses `ReplaceAllString`.
    // Heuristic: if arg[1]'s emit references a map-shape identifier
    // we can't tell from here — pick by `Expr` shape (Const →
    // hash lookup, StringInterp / Lit::Str → string replacement).
    // For json_builder's `s.gsub(ESCAPE_PATTERN, ESCAPES)` both
    // args are Const, so we land on the hash-lookup form.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            let recv_s = emit_expr(ctx, r);
            let pattern = emit_expr(ctx, &args[0]);
            let replacement = emit_expr(ctx, &args[1]);
            let is_string_repl = matches!(
                &*args[1].node,
                ExprNode::Lit { value: Literal::Str { .. } }
                    | ExprNode::StringInterp { .. }
            );
            if is_string_repl {
                return format!("{pattern}.ReplaceAllString({recv_s}, {replacement})");
            }
            return format!(
                "{pattern}.ReplaceAllStringFunc({recv_s}, func(m string) string {{ \
                 return {replacement}[m] }})"
            );
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

    // `<Const>.new(args)` → `New<Const>(args)`. The constructor is
    // synthesized by `library::emit_constructor` for any class with
    // an `initialize` method; this rewrite makes the Ruby `.new`
    // call site target it.
    if method == "new" {
        if let Some(r) = recv {
            if let ExprNode::Const { path } = &*r.node {
                let class_name = path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                return format!("New{class_name}({})", args_s.join(", "));
            }
        }
    }

    // Ruby→Go method-name mapping for string operations that have no
    // 1:1 in Go's stdlib (`strip` is `strings.TrimSpace(…)`, not
    // `.Strip()`). Applied whenever the method name matches the
    // known str-specific set — we don't gate on receiver Ty because
    // many lowered Var receivers come through with `Ty::Untyped`
    // (signature said `untyped`) or with no analyzer-set Ty at all.
    // A wrong hit (e.g. an Array's `include?` rendering as
    // `strings.Contains`) emits invalid Go and surfaces the gap;
    // silently emitting `.Include(...)` would produce an undefined-
    // method error that's harder to debug.
    if let Some(r) = recv {
        let recv_s = emit_expr(ctx, r);
        if args.is_empty() {
            if let Some(wrapped) = map_go_str_method(method, &recv_s) {
                return wrapped;
            }
        }
        if args.len() == 1 {
            if let Some(wrapped) = map_go_str_method_1arg(method, &recv_s, &args_s[0]) {
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
            // `Const(X).method(args)` → `X_method(args)`. Go has no
            // method-dispatch syntax on a type (vs. an instance), so
            // class methods live as bare functions under the
            // `<Class>_<method>` convention that matches what
            // `library::emit_method` produces for `def self.X`. The
            // sanitize-and-snake form preserves Ruby `?`/`!`/`=`
            // suffixes the same way method-decl emit does. The
            // `.new` special case (above) intercepts constructor
            // dispatch before this fires.
            if let ExprNode::Const { path } = &*r.node {
                let class_name = path
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                let method_sanitized = super::library::sanitize_method_name(method);
                return format!(
                    "{class_name}_{method_sanitized}({})",
                    args_s.join(", "),
                );
            }
            let recv_s = emit_expr(ctx, r);
            // Struct field access vs method call: 0-arg Sends on a
            // non-Class receiver whose method isn't a known AR/stdlib
            // call render without parens (`p.Title`, not `p.Title()`).
            if args_s.is_empty() && !is_known_go_method(method) {
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

/// Result of recognizing an `if recv.is_a?(Class)` shape and
/// mapping it to Go's type-assert if-init form. Carries everything
/// the per-If handler needs to splice in the init clause AND
/// substitute uses of the original receiver inside the then_branch.
pub(super) struct IsAInit {
    /// `s, ok := v.(string); ` — pre-cond text dropped into the
    /// `if`'s init slot. Ends with `; ` so the caller can concat
    /// straight onto the cond.
    pub init: String,
    /// Cond text — always `"ok"` today (the assertion's bool).
    pub cond: &'static str,
    /// Original receiver's Var name, if the receiver was a bare
    /// Var. The then_branch's child ctx adds this → asserted_ident
    /// to its rename map. `None` for non-Var receivers (`foo().is_a?`)
    /// — those emit the init but skip the rewrite.
    pub recv_name: Option<String>,
    /// Asserted identifier (`s`/`i`/`f`/...) — the new name bound
    /// in the if's scope.
    pub asserted_ident: &'static str,
}

/// Build the `IsAInit` for an `is_a?` predicate that has a mapped
/// Go assertion type. Returns `None` if either the shape or the
/// class isn't supported, so callers can fall through to the
/// unchanged path.
fn try_emit_is_a_init(ctx: &EmitCtx, cond: &Expr) -> Option<IsAInit> {
    let (recv, class) = is_a_predicate(cond)?;
    let go_ty = ruby_class_to_go_assert_ty(class)?;
    let recv_s = emit_expr(ctx, recv);
    let asserted_ident = assert_ident_for(go_ty);
    let recv_name = match &*recv.node {
        ExprNode::Var { name, .. } => Some(name.as_str().to_string()),
        _ => None,
    };
    Some(IsAInit {
        init: format!("{asserted_ident}, ok := {recv_s}.({go_ty}); "),
        cond: "ok",
        recv_name,
        asserted_ident,
    })
}

/// Pick a short, idiomatic Go identifier for the asserted typed
/// value. Single-letter conventions: `s` for strings, `i` for
/// ints, `f` for floats. Falls back to `narrowed` for anything
/// else (won't currently hit, but keeps the surface total).
fn assert_ident_for(go_ty: &str) -> &'static str {
    match go_ty {
        "string" => "s",
        "int64" => "i",
        "float64" => "f",
        _ => "narrowed",
    }
}

/// Result of recognizing the `return X if var.nil?` head of a
/// method body whose `var` has a `Union { Nil, T }` type — the rest
/// of the body sees `var` narrowed to `T`. Drives a runtime type
/// assertion + rename so subsequent uses see the typed value.
struct NilNarrow {
    /// Original Var name (`s` in `return … if s.nil?`).
    recv_name: String,
    /// Non-Nil Go assertion type the union collapsed to (`string`,
    /// `int64`, ...).
    go_ty: &'static str,
    /// New identifier bound to the asserted value (`s_str` for
    /// String-narrowed `s`).
    narrowed_ident: String,
}

/// Recognize `If { cond: var.nil?, then: Return, else: Nil }` at
/// the head of a `Seq` body and, when `var`'s Ty is a Union with
/// exactly one non-Nil variant we can map to Go, return the
/// narrowing plan. None when the shape doesn't match — caller
/// emits the head expr normally and the rest unchanged.
fn try_nil_narrow_head(first: &Expr) -> Option<NilNarrow> {
    let ExprNode::If { cond, then_branch, else_branch } = &*first.node else {
        return None;
    };
    if !is_nil_lit(else_branch) {
        return None;
    }
    // then_branch must be a Return (the early-out shape we're after).
    if !matches!(&*then_branch.node, ExprNode::Return { .. }) {
        return None;
    }
    let ExprNode::Send { recv, method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "nil?" || !args.is_empty() {
        return None;
    }
    let r = recv.as_ref()?;
    let ExprNode::Var { name, .. } = &*r.node else {
        return None;
    };
    let Some(Ty::Union { variants }) = r.ty.as_ref() else {
        return None;
    };
    let non_nil: Vec<&Ty> = variants
        .iter()
        .filter(|t| !matches!(t, Ty::Nil))
        .collect();
    if non_nil.len() != 1 {
        return None;
    }
    let go_ty = match non_nil[0] {
        Ty::Str | Ty::Sym => "string",
        Ty::Int => "int64",
        Ty::Float => "float64",
        Ty::Bool => "bool",
        _ => return None,
    };
    let short = match go_ty {
        "string" => "str",
        "int64" => "int",
        "float64" => "f64",
        "bool" => "b",
        _ => "v",
    };
    let recv_name = name.as_str().to_string();
    let narrowed_ident = format!("{recv_name}_{short}");
    Some(NilNarrow { recv_name, go_ty, narrowed_ident })
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
    // Ruby's `if`/`case` are expressions; Go's are statements. When
    // an Assign's value is one of those expression-bearing
    // statement-shapes, wrap in an immediately-invoked closure so
    // the result lands in the bound variable. `interface{}` keeps
    // the return type total — caller-side uses see `interface{}`
    // and downstream emits (Sprintf %v) already accept that.
    let v = if matches!(&*value.node, ExprNode::If { .. } | ExprNode::Case { .. }) {
        let body = emit_return_body(ctx, value);
        let indented = body
            .lines()
            .map(|l| format!("\t{l}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("func() interface{{}} {{\n{indented}\n}}()")
    } else {
        emit_expr(ctx, value)
    };
    match target {
        LValue::Var { name, .. } => {
            // First assignment to this name → declare (`:=`).
            // Subsequent assignments → reassign (`=`). Matches Ruby's
            // flat function scope: Ruby `x = 1` inside an if-block
            // reassigns the outer `x` rather than shadowing it, which
            // is what we want emitted in Go (otherwise the write is
            // silently lost when the inner scope exits).
            let name_s = name.as_str().to_string();
            let first = ctx.declared.borrow_mut().insert(name_s.clone());
            if first {
                format!("{name_s} := {v}")
            } else {
                format!("{name_s} = {v}")
            }
        }
        LValue::Ivar { name } => {
            // Same scoping rule as Ivar reads: module-singleton
            // writes target the namespaced `<Class>_<ivar>_slot`
            // package var; bare class methods write to the bare
            // lowercase package var; instance methods write through
            // `self.Field` (PascalCased).
            if ctx.in_module_singleton {
                if let Some(class) = ctx.class_name.as_deref() {
                    format!("{class}_{}_slot = {v}", name.as_str())
                } else {
                    format!("{} = {v}", name.as_str())
                }
            } else if ctx.in_class_method {
                format!("{} = {v}", name.as_str())
            } else {
                let pascal = go_field_name(name.as_str());
                format!("self.{pascal} = {v}")
            }
        }
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

/// Single-argument Ruby string methods → Go stdlib. Receiver must be
/// `Ty::Str` for these to apply (caller-side gate).
fn map_go_str_method_1arg(method: &str, recv: &str, arg: &str) -> Option<String> {
    match method {
        "split" => Some(format!("strings.Split({recv}, {arg})")),
        "start_with?" => Some(format!("strings.HasPrefix({recv}, {arg})")),
        "end_with?" => Some(format!("strings.HasSuffix({recv}, {arg})")),
        "include?" => Some(format!("strings.Contains({recv}, {arg})")),
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
        Literal::Str { value } => go_str_literal(value),
        Literal::Sym { value } => go_str_literal(value.as_str()),
        Literal::Regex { pattern, flags } => {
            // Empty flags → bare pattern (Go's `(?)` is a parse
            // error; the `(?<flags>)` group requires at least one
            // flag char). With flags, prepend the standard prefix.
            let go_pattern = ruby_regex_to_go(pattern);
            let full = if flags.is_empty() {
                go_pattern
            } else {
                format!("(?{flags}){go_pattern}")
            };
            format!("regexp.MustCompile({})", go_str_literal(&full))
        }
    }
}

/// Translate Ruby regex source to a Go-acceptable pattern. The two
/// regression points hit so far:
///
/// - `\b` / `\f` inside a character class — Ruby treats these as
///   backspace / form-feed (control chars); Go's regexp rejects them
///   as "invalid escape sequence" inside `[]` (interprets `\b` only
///   as word boundary, valid only outside `[]`). Rewrite both to
///   the explicit hex form (`\x08` / `\x0c`) when seen inside `[]`.
///
/// Other Ruby regex extensions (named captures, lookbehind, etc.)
/// pass through unchanged for now; surface when they show up.
fn ruby_regex_to_go(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars().peekable();
    let mut in_class = false;
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(&next) = chars.peek() {
                    if in_class && next == 'b' {
                        out.push_str("\\x08");
                        chars.next();
                        continue;
                    }
                    if in_class && next == 'f' {
                        out.push_str("\\x0c");
                        chars.next();
                        continue;
                    }
                    out.push('\\');
                    out.push(next);
                    chars.next();
                } else {
                    out.push('\\');
                }
            }
            '[' => {
                in_class = true;
                out.push('[');
            }
            ']' => {
                in_class = false;
                out.push(']');
            }
            other => out.push(other),
        }
    }
    out
}

/// Emit a Go-syntactic double-quoted string literal. Rust's `{:?}`
/// produces `\u{8}` / `\u{c}` for backspace / form-feed which Go
/// rejects (`U+007B '{' illegal in escape sequence`); Go uses `\b`
/// `\f` plus the fixed-width `\xHH` / `\uHHHH` / `\UHHHHHHHH`
/// forms. Covers all controls + the standard escapable chars.
fn go_str_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{07}' => out.push_str("\\a"),
            '\u{0b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c if (c as u32) < 0x7F => out.push(c),
            c if (c as u32) < 0x10000 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push_str(&format!("\\U{:08x}", c as u32)),
        }
    }
    out.push('"');
    out
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
            let (init, cond_s, then_ctx) = match try_emit_is_a_init(ctx, cond) {
                Some(IsAInit { init, cond, recv_name, asserted_ident }) => {
                    let child = match recv_name {
                        Some(n) => ctx.with_rename(n, asserted_ident.to_string()),
                        None => ctx.clone(),
                    };
                    (init, cond.to_string(), child)
                }
                None => (String::new(), emit_expr(ctx, cond), ctx.clone()),
            };
            indent(out, depth);
            out.push_str(&format!("if {init}{cond_s} {{\n"));
            emit_return_at(&then_ctx, then_branch, out, depth + 1);
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
            // the return-position expression. Special-case: when the
            // first expr is `return X if var.nil?` and `var`'s Ty is
            // `Union { Nil, T }`, narrow `var` to `T` for the rest
            // of the Seq via runtime type assertion + rename.
            let narrow = exprs.first().and_then(try_nil_narrow_head);
            let mut tail_ctx_cell: Option<EmitCtx> = None;
            for (i, sub) in exprs.iter().enumerate() {
                let is_last = i + 1 == exprs.len();
                // Switch to the narrowed child ctx for everything
                // after the early-return head, once we've emitted it.
                let active_ctx = if i == 0 || tail_ctx_cell.is_none() {
                    ctx
                } else {
                    tail_ctx_cell.as_ref().unwrap()
                };
                if is_last {
                    emit_return_at(active_ctx, sub, out, depth);
                } else {
                    indent(out, depth);
                    out.push_str(&emit_expr(active_ctx, sub));
                    out.push('\n');
                }
                // After emitting the first expr, if it matched the
                // nil-narrow shape, inject the assertion + build the
                // child ctx that swaps `var` for the narrowed ident
                // in subsequent walks.
                if i == 0 {
                    if let Some(n) = &narrow {
                        indent(out, depth);
                        out.push_str(&format!(
                            "{narrowed} := {recv}.({go_ty})\n",
                            narrowed = n.narrowed_ident,
                            recv = n.recv_name,
                            go_ty = n.go_ty,
                        ));
                        // Track the newly-declared name so a later
                        // Assign to it emits `=`, not `:=`.
                        ctx.declared.borrow_mut().insert(n.narrowed_ident.clone());
                        tail_ctx_cell = Some(
                            ctx.with_rename(n.recv_name.clone(), n.narrowed_ident.clone()),
                        );
                    }
                }
            }
        }
        ExprNode::Return { value } => {
            // Already a return; don't double up to `return return X`.
            // Void methods elide the value entirely.
            if ctx.void_method {
                indent(out, depth);
                out.push_str("return\n");
            } else {
                let v = emit_expr(ctx, value);
                indent(out, depth);
                out.push_str(&format!("return {v}\n"));
            }
        }
        ExprNode::While { .. } => {
            // While at body position emits as a statement, not
            // wrapped in `return`. Ruby's `while` evaluates to nil
            // so the loop's value is discarded; subsequent
            // statements (or an implicit nil tail) supply the
            // function's return.
            let s = emit_expr(ctx, e);
            for line in s.lines() {
                indent(out, depth);
                out.push_str(line);
                out.push('\n');
            }
        }
        _ => {
            // Void method tails: emit the expression as a statement
            // (when it has side effects) or skip entirely for a bare
            // `nil`. Ruby's implicit-nil tail must not turn into
            // `return nil` against a Go void function.
            if ctx.void_method {
                if is_nil_lit(e) {
                    // No-op trailing nil.
                    return;
                }
                let v = emit_expr(ctx, e);
                indent(out, depth);
                out.push_str(&v);
                out.push('\n');
            } else {
                let v = emit_expr(ctx, e);
                indent(out, depth);
                out.push_str(&format!("return {v}\n"));
            }
        }
    }
}
