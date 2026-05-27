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
    current_class_method_param_tys, emit_expr, emit_send_recv, in_class_method, is_static_method,
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
    if let Some(s) = try_view_helpers_dom_id(recv, method, args) { return s; }
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
        let mut out: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, a)| coerce_arg_for_class_method(&effective_method, i, a))
            .collect();
        // Trailing default-arg pad for AC::Base controller shims —
        // Ruby `head :sym` (no kwargs) and `head :sym, content_type:`
        // (with kwargs) call the same method, but the rust2 shim has
        // fixed arity. `controller_shim_arity` reports the declared
        // count; `controller_shim_method_param_ty` gives the Ty per
        // index. `synth_default_for_ty` produces the literal
        // (`HashMap::new()` for trailing opts). Without this padding,
        // 1-arg `self.head(:not_found)` sites trip E0061 once the shim
        // signature gains the kwargs param to absorb 2-arg calls.
        if let Some(arity) = dispatch::controller_shim_arity(&effective_method) {
            for i in out.len()..arity {
                if let Some(pt) = dispatch::controller_shim_method_param_ty(&effective_method, i) {
                    if let Some(d) = super::util::synth_default_for_ty(&pt) {
                        out.push(d);
                    } else {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        out
    } else if let ExprNode::Const { path } = &*r.node {
        let class = path.last().map(|s| s.as_str()).unwrap_or("");
        // Try the hand-written runtime sigs first (Db, Broadcasts),
        // then a cross-LC global lookup keyed by (ClassName, method) —
        // covers `Comment::new(...)` called from inside any other
        // class. `current_class_method_param_tys` is the last-resort
        // fallback: it's keyed by method name only and ignores the
        // Const path, so a same-named method in the current class
        // (e.g. a view module's `new` action partial) would otherwise
        // shadow the cross-class param list and pad with the wrong
        // defaults. Trying global first inverts the precedence so the
        // explicit class qualifier wins when both tables list `new`.
        let param_tys = external_class_method_param_tys(class, method)
            .or_else(|| super::global_class_method_param_tys(class, method))
            .or_else(|| current_class_method_param_tys(method));
        // Kwargs-unpack pre-pass: when the callee declares keyword
        // params and the trailing arg is a kwargs Hash literal, expand
        // the Hash entries into positional slots by name. Without this,
        // `ViewHelpers::truncate(text, length: 100)` emits the Hash
        // literal at the `length: Integer` slot, tripping E0308. Only
        // fires when the rich-param lookup is available (global
        // registry), so call sites unknown to the registry retain the
        // existing positional fallback.
        let rich_params = super::global_class_method_params(class, method);
        let owned_args_storage: Vec<Expr>;
        let effective_args: &[Expr] = if let Some(rp) = rich_params.as_ref() {
            if let Some(unpacked) = unpack_trailing_kwargs(args, rp) {
                owned_args_storage = unpacked;
                &owned_args_storage
            } else {
                args
            }
        } else {
            args
        };
        if let Some(param_tys) = param_tys {
            let mut out: Vec<String> = Vec::with_capacity(param_tys.len().max(effective_args.len()));
            for (i, _) in param_tys.iter().enumerate() {
                match (effective_args.get(i), param_tys.get(i)) {
                    // Caller-supplied arg: apply per-param coercion.
                    (Some(a), Some(pt)) => out.push(coerce_arg_for_param_ty(a, pt)),
                    // Missing trailing arg: prefer the source-level
                    // default (e.g. Ruby `omission: "..."`) when the
                    // collected registry has one for this position;
                    // otherwise fall back to the Ty-only default
                    // (Hash → `HashMap::new()`, Str → `""`, etc.).
                    // The source-level path is what gets Rails'
                    // `truncate(text, length: 100)` to render
                    // `...`-suffixed output instead of mid-word
                    // truncation.
                    (None, Some(pt)) => {
                        if let Some(d) =
                            super::global_class_method_param_default(class, method, i)
                        {
                            out.push(d);
                        } else if let Some(d) = synth_default_for_ty(pt) {
                            out.push(d);
                        } else {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            // If caller passed MORE args than callee declares (rare —
            // splat / overload patterns), append the extras un-coerced.
            for a in effective_args.iter().skip(param_tys.len()) {
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
    let recv_s = emit_send_recv(r);
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

/// Inline `ViewHelpers::dom_id(record, suffix)` when `record`'s type
/// is a known concrete model. Rust2 emits per-model structs (Article,
/// Comment, …) but the runtime's `dom_id(record: Base, ...)` only
/// accepts the abstract `Base` struct — there's no enum/trait bridge
/// in either direction. The Ruby body is a one-liner format, so
/// inlining at the call site lets the per-model `.id()` accessor +
/// the snake_case'd class name (= the dom_prefix the lowerer
/// synthesizes for each model) carry the result directly.
///
/// Returns `None` for any non-matching shape — opaque-typed recv,
/// non-Const recv, recv-class without a Class Ty, etc. — so the
/// regular dispatch loop runs.
fn try_view_helpers_dom_id(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    if method != "dom_id" {
        return None;
    }
    let r = recv?;
    let ExprNode::Const { path } = &*r.node else { return None };
    if path.last().map(|s| s.as_str()) != Some("ViewHelpers") {
        return None;
    }
    if args.is_empty() || args.len() > 2 {
        return None;
    }
    let record = &args[0];
    let class_name = match record.ty.as_ref()? {
        crate::ty::Ty::Class { id, .. } => id.0.as_str().to_string(),
        _ => return None,
    };
    // Skip the runtime Base itself — when called with `Base`, fall
    // through to the runtime helper (the abstract method raises, but
    // that's the runtime's contract, not the peephole's concern).
    if class_name == "Base" || class_name == "ActiveRecord::Base" {
        return None;
    }
    let prefix = crate::naming::snake_case(
        class_name.rsplit("::").next().unwrap_or(class_name.as_str()),
    );
    let record_s = emit_expr(record);
    // 1-arg `dom_id(record)` → `"<prefix>_<id>"` with no suffix.
    if args.len() == 1 {
        return Some(format!(
            "format!(\"{}_{{}}\", {}.clone().id())",
            prefix, record_s,
        ));
    }
    // 2-arg `dom_id(record, suffix)`. Suffix at IR level is either a
    // literal Sym/Str (the lowerer threads `:comments_count` through
    // as-is) or — rarely — a dynamic expression. Inline the literal
    // form into the format! string for the cleanest emit; bail on
    // dynamic shapes so the call falls back to the runtime helper
    // (which we'd need to make generic for those to compile, but no
    // real-blog site hits them today).
    let suffix = &args[1];
    // Peek through Cast wrappers — `lower::ty_coerce_insertion` may have
    // wrapped the literal suffix in `Cast(arg, Option<Sym>)` since
    // `dom_id`'s second param is declared `Symbol?`. The peephole's
    // suffix-literal extraction needs to see the inner Lit shape.
    let suffix_inner = if let ExprNode::Cast { value, .. } = &*suffix.node {
        value
    } else {
        suffix
    };
    let suffix_lit: Option<&str> = match &*suffix_inner.node {
        ExprNode::Lit { value: crate::expr::Literal::Sym { value } } => Some(value.as_str()),
        ExprNode::Lit { value: crate::expr::Literal::Str { value } } => Some(value.as_str()),
        _ => None,
    };
    let suffix_lit = suffix_lit?;
    Some(format!(
        "format!(\"{}_{}_{{}}\", {}.clone().id())",
        suffix_lit, prefix, record_s,
    ))
}

/// Rewrite a positional-Hash call shape into a positional-only shape
/// when the callee declares Keyword params. Returns `None` if no
/// rewrite is applicable (no trailing kwargs Hash, or callee has no
/// keyword params), so the caller can keep its existing args list.
///
/// Triggers when:
///   * the last arg is `ExprNode::Hash { kwargs: true, … }`, AND
///   * the callee's param list (after the leading positionals already
///     supplied by the caller) contains at least one Keyword param.
///
/// For each remaining param, look up the matching entry by name in
/// the Hash; if found, push that value; if missing (kwargs Hash
/// silently omits optional kwargs), emit nothing for that slot and
/// let the existing trailing-default loop synthesize the default.
fn unpack_trailing_kwargs(
    args: &[Expr],
    params: &[crate::ty::Param],
) -> Option<Vec<Expr>> {
    use crate::expr::{ExprNode, Literal};
    use crate::ty::ParamKind;
    let last = args.last()?;
    let (entries, _) = match &*last.node {
        ExprNode::Hash { entries, kwargs } if *kwargs => (entries, true),
        _ => return None,
    };
    let positional_count = args.len() - 1;
    // Need at least one Keyword param at-or-after the supplied
    // positional count. Otherwise this is a regular trailing-Hash
    // shape and the existing positional dispatch handles it.
    let any_kw_after = params
        .iter()
        .skip(positional_count)
        .any(|p| matches!(p.kind, ParamKind::Keyword { .. }));
    if !any_kw_after {
        return None;
    }
    // Index the Hash literal's entries by key-name. Accept both Symbol
    // and String literal keys (Ruby kwargs surface either way through
    // the parser depending on call shape).
    let mut by_name: std::collections::HashMap<String, &Expr> =
        std::collections::HashMap::new();
    for (k, v) in entries.iter() {
        let name = match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            _ => return None, // dynamic key — can't unpack at emit
        };
        by_name.insert(name, v);
    }
    let mut out: Vec<Expr> = args[..positional_count].to_vec();
    for p in params.iter().skip(positional_count) {
        match p.kind {
            ParamKind::Keyword { .. } => {
                if let Some(v) = by_name.get(p.name.as_str()) {
                    out.push((*v).clone());
                } else {
                    // Missing kwarg: stop pushing so the caller's
                    // trailing-default-synth loop fills in. Required
                    // kwargs (`required: true`) would surface as a
                    // missing-arg compile error in Rust, which is the
                    // right outcome — the Ruby source is missing the
                    // required keyword.
                    break;
                }
            }
            ParamKind::Required | ParamKind::Optional => {
                // Positional param past the supplied count and before
                // any Keyword params — the caller didn't supply it, so
                // leave the slot for the trailing-default loop.
                break;
            }
            _ => break,
        }
    }
    Some(out)
}

