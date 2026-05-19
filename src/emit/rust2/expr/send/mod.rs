//! Method-call emit — `ExprNode::Send` and its support helpers.
//! Routes call sites by receiver type, applies arg coercion (against
//! callee param types when known), and bridges Ruby stdlib methods to
//! their Rust analogues. The bulk of expr/ emit lives here; the file
//! is large by necessity because `Send` covers method calls, operator
//! desugars, indexing, and most Ruby-stdlib bridging.

use crate::expr::{Expr, ExprNode};

mod coerce;
mod dispatch;
mod index;
mod ops;

pub(crate) use coerce::coerce_arg_for_param_ty;
pub(super) use coerce::{cast_via_value_for_union, coerce_arg_for_field_ty};

use coerce::coerce_arg_for_class_method;
use dispatch::external_class_method_param_tys;
use index::try_recv_typed_method;
use ops::{
    try_array_push, try_binary_operator, try_constructor_field_assign,
    try_stdlib_class_method, try_string_append, try_unary_not,
};

use super::util::{rewrite_method_name, synth_default_for_ty};
use super::{
    current_class_method_param_tys, emit_expr, in_class_method, is_static_method,
};

pub(super) fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    outer_ty: Option<&crate::ty::Ty>,
) -> String {
    // Ruby implicit-self resolves a bare identifier to the enclosing
    // method's parameter when one shares the name (e.g. view partial
    // `def self.article(article, ...)` body references `article` as
    // the local, not as `Articles::article` recursion). The view
    // lowerer emits these as `Send { recv: None, method, args: [] }`
    // — same shape as a zero-arg static method call. Without this
    // filter, `ViewHelpers::dom_id(article)` emits as
    // `ViewHelpers::dom_id(article())`, which Rust rejects with
    // E0618 (expected function, found `Article`). Match the
    // enclosing-param-name shape and emit the bare Var read.
    //
    // Mirrors `is_enclosing_param_name` in `src/emit/typescript/expr.rs`.
    //
    // Apply the same narrowing-write-back that `ExprNode::Var` reads
    // use: when the body-typer narrowed the param from `Option<T>` to
    // `T` (e.g. inside an `if !notice.nil? && !notice.empty?` body),
    // emit `notice.clone().unwrap()` so the downstream `&str` site
    // sees `String` (auto-derefs via `&`). Without this, the Send
    // shape stayed as a bare `notice` while `Var` reads got unwrapped,
    // breaking calls like `html_escape(&(notice))`.
    if recv.is_none() && args.is_empty() && super::param_ty(method).is_some() {
        if let Some(s) = super::narrowed_param_read(method, outer_ty) {
            return s;
        }
        return super::util::sanitize_ident(method);
    }
    if let Some(s) = try_constructor_field_assign(recv, method, args) { return s; }
    if let Some(s) = try_stdlib_class_method(recv, method, args) { return s; }
    if let Some(s) = try_binary_operator(recv, method, args) { return s; }
    if let Some(s) = try_unary_not(recv, method, args) { return s; }
    if let Some(s) = try_array_push(recv, method, args) { return s; }
    if let Some(s) = try_string_append(recv, method, args) { return s; }
    if let Some(s) = try_recv_typed_method(recv, method, args) { return s; }
    // Ruby/Rust method-name bridge. Sanitize predicates (`foo?` →
    // `foo`, `foo!` → `foo`) since Rust identifiers reject those
    // suffixes. The user-defined HWIA methods `key?`/`has_key?`/etc.
    // pair with the matching `pub fn` rename in `method.rs` so def
    // and call sites stay aligned. A small set of Ruby stdlib calls
    // (`to_s`, `length`, `nil?`, `key?` on Hash, etc.) needs a
    // different Rust name; rewrite those here. Caveat: receiver-type-
    // sensitive bridges (Hash#key? vs user-defined `key?`) collapse
    // to the generic form — Rust's `contains_key` for HashMap vs
    // the user's stripped `key` may emit ambiguously when the recv
    // is untyped serde_json::Value. Live with the noise until type-
    // aware bridging lands.
    // Arity-disambiguate `render` for AC::Base shim:
    //   self.render(content)            → self.render(content)
    //   self.render(content, opts_hash) → self.render_with(content, opts)
    // Rust forbids two methods of the same name with different
    // arities, but Ruby's `render` is overloaded by call shape. The
    // shim provides both methods under different names; this rewrite
    // routes the 2-arg form to `render_with`. Conservative — only
    // applies when recv is SelfRef (in a controller body) and method
    // is exactly "render". Other render-named methods on other
    // recvs (e.g. a hypothetical `template.render`) stay unchanged.
    let effective_method: String = if method == "render"
        && args.len() == 2
        && matches!(recv, Some(r) if matches!(&*r.node, ExprNode::SelfRef))
    {
        "render_with".to_string()
    } else {
        method.to_string()
    };
    let rewritten_method = rewrite_method_name(&effective_method);
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Free functions / module functions (Inflector.pluralize → bare
    // pluralize() in the inflector module). Implicit-self bare calls
    // emit as bare function calls.
    if recv.is_none() {
        // `require "X"` inside a method body — Ruby's lazy load
        // statement. Rust resolves cross-file deps through top-level
        // `use` imports (the runtime_loader's `imports` field), so
        // the inline `require` has nothing to do at runtime. Emit as
        // a comment so the line stays inert.
        if method == "require" {
            let arg_repr = args_s.join(", ");
            return format!("/* require({arg_repr}) — no-op in rust2 */");
        }
        // Ruby's class-method `new` (implicit-self call to Class#new
        // inside a `def self.X` body). Lowers to `Send { recv: None,
        // method: "new" }`. Rust analog inside an `impl Type` is
        // `Self::new(args)` — the constructor's canonical Rust name
        // (matches `emit_instance_method`'s `is_init` lowering).
        if method == "new" && in_class_method() {
            return format!("Self::new({})", args_s.join(", "));
        }
        return format!("{}({})", rewritten_method, args_s.join(", "));
    }
    let r = recv.unwrap();
    // Static-method routing: `self.method(args)` where `method` was
    // classified as not-reading-self emits as `Self::method(args)`.
    // Required inside `pub fn new` (no instance yet), and also a
    // valid choice elsewhere for inherently-static helpers — Rust
    // accepts both `obj.foo()` and `T::foo(...)` when `foo` doesn't
    // take a receiver, but the static form is unambiguous.
    //
    // The same routing applies unconditionally inside class methods
    // (`def self.X` bodies): Ruby's `self` *is* the class there, so
    // every `self.method(args)` is class-level dispatch.
    if matches!(&*r.node, ExprNode::SelfRef) && (is_static_method(method) || in_class_method()) {
        // Callee-back-propagation: when the callee's declared param[i]
        // is `Hash<K, V>` and the arg expression is a Var whose
        // `local_var_ty` is a different `Hash<K', V'>` (or
        // body-typer-derived but with mismatched K/V), insert a
        // `into_iter().map().collect()` transform. The button_to →
        // render_attrs(form_attrs) pattern is the canonical case:
        // form_attrs is locally `HashMap<&str, String>` (from
        // `{action: …, method: "post"}.to_h`), render_attrs takes
        // `HashMap<String, serde_json::Value>`.
        let coerced: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, a)| coerce_arg_for_class_method(&effective_method, i, a))
            .collect();
        if coerced.is_empty() {
            return format!("Self::{rewritten_method}()");
        }
        return format!("Self::{rewritten_method}({})", coerced.join(", "));
    }
    // Callee-back-propagation for two recv shapes:
    //
    // 1. **SelfRef instance method** (`self.set_id(arg)`): callee is
    //    a sibling method on the current class. Use
    //    `CLASS_METHOD_PARAM_TYS` (populated by `library.rs` at class
    //    emit start) to look up the param Tys. Closes the lowered
    //    model `self.set_id(row["id"])` shape (Value → i64 coercion).
    // 2. **Const class method** (`Db::escape_string(self.body)`):
    //    callee is in a hand-written runtime module not surfaced
    //    through the per-class registry. Hardcoded
    //    `external_class_method_param_tys` covers Db today; future
    //    modules add entries as their sites surface.
    let final_args: Vec<String> = if matches!(&*r.node, ExprNode::SelfRef) {
        args.iter()
            .enumerate()
            .map(|(i, a)| coerce_arg_for_class_method(&effective_method, i, a))
            .collect()
    } else if let ExprNode::Const { path } = &*r.node {
        let class = path.last().map(|s| s.as_str()).unwrap_or("");
        // Try the hand-written runtime sigs first (Db, Broadcasts),
        // then fall back to the current class's method table. The
        // latter covers `Self::new(...)` / `Class::method(...)` shapes
        // where path.last() matches the class currently being
        // emitted — `Article::new()` from inside `impl Article`.
        let param_tys = external_class_method_param_tys(class, method)
            .or_else(|| current_class_method_param_tys(method))
            .or_else(|| super::global_class_method_param_tys(class, method));
        if let Some(param_tys) = param_tys {
            let mut out: Vec<String> = Vec::with_capacity(param_tys.len().max(args.len()));
            for (i, _) in param_tys.iter().enumerate() {
                match (args.get(i), param_tys.get(i)) {
                    // Caller-supplied arg: apply per-param coercion.
                    (Some(a), Some(pt)) => out.push(coerce_arg_for_param_ty(a, pt)),
                    // Missing trailing arg: synthesize a default for
                    // shapes that have one (Hash → HashMap::new(),
                    // etc.). Ruby `def initialize(attrs = {})`
                    // accepts zero-arg `Article.new`; Rust needs the
                    // explicit default. Without this `Class::new()`
                    // sites trip E0061.
                    (None, Some(pt)) => match synth_default_for_ty(pt) {
                        Some(d) => out.push(d),
                        None => break,
                    },
                    _ => break,
                }
            }
            // If caller passed MORE args than callee declares (rare —
            // splat / overload patterns), append the extras un-coerced.
            for a in args.iter().skip(param_tys.len()) {
                out.push(emit_expr(a));
            }
            out
        } else {
            args_s
        }
    } else if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Class { .. }))
        && method.ends_with('=')
        && args.len() == 1
    {
        // Setter convention coercion for instance-Send recvs whose Ty
        // is a known class: `instance.set_<col>(value)` came from the
        // model lowerer's `attr_writer` shim, where the setter param
        // ty equals the column ty. rust2 emits `Ty::Str/Sym` params
        // as `&str`, but the lowerer hands String-shaped args at
        // sites like `instance.set_body(row.body())` (where
        // `row.body()` returns owned `String`). Wrap owned-String
        // sources with `&(...)` so the borrow matches. Non-Str
        // setter args (`set_id(i64)`) pass through unchanged.
        //
        // Heuristic — the Const-recv arm above uses an explicit
        // param-Tys table; for instance-Sends we don't have a global
        // sibling registry yet, so the setter-name + arg-Ty + owned-
        // node combo carries the same signal. Limited to one-arg
        // calls because that's the AR `set_<col>` shape; broader
        // setter shapes (multi-arg) can opt in later.
        let mut out: Vec<String> = Vec::with_capacity(1);
        let coerced = coerce_arg_for_param_ty(
            &args[0],
            // Use the arg's body-typer Ty as the param Ty: setter
            // params for Str cols are typed Str, matching the row
            // accessor's return Ty. For non-Str args the coerce
            // function returns the bare emit.
            args[0].ty.as_ref().unwrap_or(&crate::ty::Ty::Untyped),
        );
        out.push(coerced);
        out
    } else {
        args_s
    };
    let recv_s = emit_expr(r);
    // Static method dispatch — `Type.method(args)` in Ruby becomes
    // `Type::method(args)` in Rust when the receiver is a Const
    // (class/module reference). The `.` form binds to a value
    // receiver; `::` binds to a type.
    let dispatch = if matches!(&*r.node, ExprNode::Const { .. }) {
        "::"
    } else {
        "."
    };
    if final_args.is_empty() {
        format!("{recv_s}{dispatch}{rewritten_method}()")
    } else {
        format!("{recv_s}{dispatch}{rewritten_method}({})", final_args.join(", "))
    }
}

