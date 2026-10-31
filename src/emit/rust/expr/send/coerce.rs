//! Callee-back-propagation arg coercion. Given an `Expr` and the
//! callee's declared param `Ty` (from a class-method table, a struct
//! field, or a Cast target), insert the appropriate Value→primitive
//! / primitive→Value / String→&str / HashMap-remap transform so the
//! emitted call type-checks. Three entry points:
//!
//!   - [`coerce_arg_for_class_method`] — looks up the param Ty from
//!     `class_method_param_ty` then defers to the core coercion fn.
//!   - [`coerce_arg_for_param_ty`] — core fn, exported so siblings
//!     (`assign.rs`, `literal.rs`, `emit_expr_inner`'s arms) can call
//!     it directly when they already know the target Ty.
//!   - [`cast_via_value_for_union`] / [`coerce_arg_for_field_ty`] —
//!     field-position variants used by the Cast arm and the
//!     constructor `self.field = value` rewrite.

use crate::expr::{Expr, ExprNode, Literal};

use super::super::util::{is_option_ty, peel_nil, ty_contains_untyped};
use super::super::{arg_hash_var_local_ty, class_method_param_ty, emit_expr};

/// Apply callee-back-propagation coercion for a single arg in a
/// class-/instance-method call where the callee is in
/// `CLASS_METHOD_PARAM_TYS`. Defers to `coerce_arg_for_param_ty`.
///
/// Two-step lookup: current-class first (sibling method on the same
/// LC), then the `controller_shim_method_param_ty` table for the
/// per-controller AC::Base shim methods (`redirect_to`, `render_with`)
/// whose signatures live as hand-coded text in `rust.rs::emit` and
/// thus aren't reachable through any LC-based registry.
pub(super) fn coerce_arg_for_class_method(method: &str, idx: usize, arg: &Expr) -> String {
    let param_ty = class_method_param_ty(method, idx)
        .or_else(|| super::dispatch::controller_shim_method_param_ty(method, idx));
    let Some(param_ty) = param_ty else {
        return emit_expr(arg);
    };
    coerce_arg_for_param_ty(arg, &param_ty)
}

/// Core callee-back-propagation coercion: given an arg's `Expr` and
/// the callee's declared param `Ty`, return the emit string with the
/// appropriate coercion applied. Four families:
///
/// 1. **HashMap shape transform**: callee `Hash<_, Untyped>` with arg
///    `Hash<_, *>` of differing K/V → wrap with `.into_iter().map().
///    collect()` into `HashMap<String, Value>`.
/// 2. **Value → primitive**: callee `Str|Int|Bool|Float` with arg's
///    body-typer Ty (post-Nil peel) `Untyped` (Value) → append
///    `.as_X().unwrap()` via `value_narrowing_coercion`.
/// 3. **Primitive → Value**: callee `Untyped`, arg a concrete
///    primitive — wrap with `Value::from(...)`.
/// 4. **String → &str (Borrow)**: callee `Str|Sym` (rust emits
///    `&str` for these param positions) with arg from a non-literal
///    String-producing source (Var/Send/Ivar) → `&(raw)`.
pub(crate) fn coerce_arg_for_param_ty(arg: &Expr, param_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    // Bare `nil` into an Untyped/Value param is JSON null, not
    // `Option::None` (AssociationReflection.new(name, nil) — #832).
    if matches!(&*arg.node, ExprNode::Lit { value: Literal::Nil })
        && super::super::super::ty::rust_value_shaped(param_ty)
    {
        return "serde_json::Value::Null".to_string();
    }
    // Value → primitive narrowing at Send arg position: when arg is
    // `Cast(inner, primitive_ty)` from the `lower::ty_coerce_insertion`
    // Family 2, and inner's body-typer Ty contains Untyped, use the
    // arg-position coercion (`(raw).as_str().unwrap()`) — NOT
    // emit_expr's Cast arm (which goes through `coerce_arg_for_field_ty`
    // producing the owned-String shape `as_str().unwrap().to_string()`
    // appropriate for field assigns but wasteful at `&str` arg
    // positions). Pre-Stage-6 unwrapped args hit this same path via
    // Family 2 below — peek-through here keeps the emit identical.
    if let ExprNode::Cast { value, target_ty } = &*arg.node {
        let inner_untyped = value.ty.as_ref().map(ty_contains_untyped).unwrap_or(false);
        if target_ty.is_scalar() && inner_untyped {
            if target_ty.is_stringish() {
                if json_value_arg_for_string_param(value) {
                    let inner_raw = emit_expr(value);
                    return json_value_as_str(inner_raw, value);
                }
                // Class-method Sends can be IR-Untyped while rust emits
                // `String`. Borrow rather than calling Value::as_str.
                return format!("&({})", emit_expr(value));
            }
            if let Some(coerce) = super::super::util::value_narrowing_coercion(target_ty) {
                let inner_raw = emit_expr(value);
                return format!("({inner_raw}).{coerce}");
            }
        }
        // Stringish unions (`String | Symbol`) are not `is_scalar` but
        // rust emits them as `&str` params. Only Hash/Value bags
        // narrow with `as_str()`; see `json_value_arg_for_string_param`.
        if target_ty.is_stringish() && json_value_arg_for_string_param(value) {
            let inner_raw = emit_expr(value);
            return json_value_as_str(inner_raw, value);
        }
    }
    // Not `vec!["a"]`: that is a Vec<&str>, and a String-element parameter takes owned Strings
    if let (ExprNode::Array { elements, .. }, Ty::Array { elem }) = (&*arg.node, peel_nil(param_ty)) {
        if elem.is_stringish() && !elements.is_empty() && elements.iter().all(|e| matches!(&*e.node, ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } })) {
            let items: Vec<String> = elements.iter().map(|e| format!("{}.to_string()", emit_expr(e))).collect();
            return format!("vec![{}]", items.join(", "));
        }
    }
    // A number into a nilable parameter of several types: rust renders it Option<Value>, which takes the number boxed.
    // Only when it does: a flat `Integer | String | nil` renders a bare Value, and `Some(..)` there cannot unify.
    if let Ty::Union { variants } = param_ty {
        let non_nil: Vec<&Ty> = variants.iter().filter(|v| !matches!(v, Ty::Nil)).collect();
        let several = non_nil.len() > 1 || matches!(non_nil.as_slice(), [Ty::Union { .. }]);
        if variants.iter().any(|v| matches!(v, Ty::Nil))
            && several
            && !super::super::super::ty::rust_value_shaped(param_ty)
            && matches!(arg.ty.as_ref(), Some(Ty::Int | Ty::Float))
        {
            return format!("Some(serde_json::Value::from({}))", emit_expr(arg));
        }
    }
    let raw = emit_expr(arg);
    let arg_ty_peeled = arg.ty.as_ref().map(peel_nil);

    // Family 6: T → Option<T> Some-wrap. When the callee param is
    // `Option<U>` (rust's emit shape for RBS-declared `U?` / `T |
    // nil`) and the arg's RAW body-typer Ty matches the peeled inner
    // U exactly, wrap with `Some(...)`. Closes
    // `JsonBuilder::encode_datetime(article.created_at())` where the
    // model attribute returns owned `String` and the callee declared
    // `String?`.
    //
    // Two gates:
    //
    // 1. RAW arg.ty (not peeled) must equal the inner. An arg whose
    //    own type is already `Option<U>` (e.g. `self.flash.get
    //    ("notice")` → `Option<String>`) must NOT get double-wrapped
    //    to `Option<Option<U>>`. The body-typer already records the
    //    matching `Option<U>` shape, so the bare emit type-checks.
    //
    // 2. Arg must be from an owned-producing node (Var/Send/Ivar).
    //    Literal-Str args (e.g. `ViewHelpers::dom_id(article,
    //    "comments_count")` reaching `Option<String>`) emit as
    //    `&'static str`, not owned `String`, so `Some(&str)` =
    //    `Option<&str>` would mismatch `Option<String>`. Closing
    //    that needs an inner `.to_string()` too — out of scope for
    //    this wedge.
    if is_option_ty(param_ty) {
        // Owned `Option<String>` field → borrowed `Option<&str>` param.
        // rust renders a nilable string PARAM as `Option<&str>` but a
        // nilable string FIELD as `Option<String>`, so passing a
        // nullable column straight into `Db::escape_string_opt` needs
        // the borrow — and `.as_deref()` gives it without moving out of
        // `&self`.
        if peel_nil(param_ty).is_stringish()
            && matches!(&*arg.node, ExprNode::Ivar { .. })
            && arg
                .ty
                .as_ref()
                .map(|t| is_option_ty(t) && peel_nil(t).is_stringish())
                .unwrap_or(false)
        {
            return format!("{raw}.as_deref()");
        }
        // Family 6 — T → Option<T> Some-wrap. The
        // `lower::ty_coerce_insertion` lowerer wraps eligible Send arg
        // positions in `Cast { value, target_ty: Option<U> }` across
        // every recv shape (Const, SelfRef, implicit-self, Var(Class),
        // Ivar(Class)) and across category boundaries (via
        // `insert_ty_coercions_with_extras`). The Cast IS the
        // decision; render trusts it and emits `Some(inner)`.
        //
        // Inner needs `.to_string()` for `Lit::Str | Lit::Sym →
        // Option<String>`: the literal emits as `&'static str` and
        // `Some(&str) = Option<&str>` would mismatch `Option<String>`.
        //
        // The OPTION_WRAP decide-pass bit + walker that used to back
        // up the Cast were retired alongside the lowerer's Var-recv
        // extension — the lowerer subsumes their coverage. The bare
        // IR-inspection fallback (owned-producing match + literal-Str
        // match) was retired earlier; both branches are subsumed by
        // the same Cast wrapper.
        if let ExprNode::Cast { value, target_ty } = &*arg.node {
            if let Some(cast_inner) = is_option_ty(target_ty).then(|| peel_nil(target_ty)) {
                // Already nilable — a nullable column's Row reader
                // returns `Option<T>` and the model setter takes the
                // same, so `Some(..)` here would build `Option<Option
                // <T>>`. The serde_json case is a different bridge:
                // an untyped Value narrows with `as_str().map(..)`,
                // which yields the Option directly.
                if value.ty.as_ref().map(ty_contains_untyped).unwrap_or(false) {
                    if let Some(c) = value_narrowing_coercion_opt(cast_inner) {
                        return format!("({}).{c}", emit_expr(value));
                    }
                }
                if value
                    .ty
                    .as_ref()
                    .map(|t| is_option_ty(t) && peel_nil(t) == cast_inner)
                    .unwrap_or(false)
                {
                    return emit_expr(value);
                }
                let inner_raw = emit_expr(value);
                let needs_to_string = matches!(
                    &*value.node,
                    ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                ) && cast_inner.is_stringish()
                    && !super::super::has_str_coercion(value);
                let payload = if needs_to_string {
                    format!("{inner_raw}.to_string()")
                } else {
                    inner_raw
                };
                return format!("Some({payload})");
            }
        }
        // Class-method / Const-recv sites the lowerer registry can
        // miss (`ActionController.header_key_ok?(key)` with String
        // vs `String?`). Rust-only; do not lift Option wrapping into
        // the shared lowerer.
        // Not the read's own type alone: a local bound to an Option keeps that type even where the analyzer narrowed the read
        let binds_option = matches!(&*arg.node, ExprNode::Var { name, .. }
            if super::super::local_var_ty(name.as_str()).is_some_and(|t| is_option_ty(&t)));
        if !matches!(&*arg.node, ExprNode::Cast { .. })
            && !arg.ty.as_ref().is_some_and(is_option_ty)
            && !binds_option
        {
            let inner = peel_nil(param_ty);
            if inner.is_stringish() {
                let needs_to_string = matches!(
                    &*arg.node,
                    ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                ) && !super::super::has_str_coercion(arg);
                let payload = if needs_to_string {
                    format!("{raw}.to_string()")
                } else if matches!(arg_ty_peeled, Some(Ty::Str | Ty::Sym))
                    || matches!(&*arg.node, ExprNode::Var { .. } | ExprNode::Send { .. })
                {
                    format!("({raw}).to_string()")
                } else {
                    raw.clone()
                };
                return format!("Some({payload})");
            }
        }
    }

    // Family 1 — Hash widening to `HashMap<String, serde_json::Value>`.
    // Two paths:
    //
    // 1. **Lowerer-inserted Cast** (the common path). When the callee
    //    param is `Hash<_, Untyped>` and the arg's Hash shape is
    //    narrower, `lower::ty_coerce_insertion::needs_hash_widening`
    //    wraps the arg in `Cast { target_ty: Hash<_, Untyped> }`. The
    //    Cast IS the decision; render trusts it.
    //
    // 2. **AC::Base shim recv fallback**. `render_with`, `redirect_to`,
    //    `head` aren't LC methods (signatures hand-coded in
    //    `dispatch.rs::controller_shim_method_param_ty`), so they
    //    aren't in the lowerer's registry — `needs_hash_widening`
    //    can't fire for `self.render_with(..., {hash})` calls. Keep an
    //    IR-inspection fallback for non-Cast args at Hash<_, Untyped>
    //    param positions, covering Var-with-local-Hash-Ty and inline
    //    Hash-literal shapes (the two arg shapes shim calls receive in
    //    practice).
    if let ExprNode::Cast { value, target_ty } = &*arg.node {
        if matches!(target_ty, Ty::Hash { value: pv, .. } if matches!(pv.as_ref(), Ty::Untyped))
            && matches!(param_ty, Ty::Hash { value: pv, .. } if matches!(pv.as_ref(), Ty::Untyped))
        {
            let inner_raw = emit_expr(value);
            return format!(
                "{inner_raw}.into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v))).collect::<std::collections::HashMap<String, serde_json::Value>>()"
            );
        }
    }
    if let Ty::Hash { value: pv, .. } = param_ty {
        if matches!(pv.as_ref(), Ty::Untyped)
            && !matches!(&*arg.node, ExprNode::Cast { .. })
        {
            if arg_hash_var_local_ty(arg).is_some()
                || matches!(&*arg.node, ExprNode::Hash { .. })
            {
                return format!(
                    "{raw}.into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v))).collect::<std::collections::HashMap<String, serde_json::Value>>()"
                );
            }
        }
    }

    // Family 2 — Value→primitive narrowing for bare Untyped args.
    // Retired: the `lower::ty_coerce_insertion::needs_value_to_primitive`
    // lowerer wraps Untyped args targeting primitive params in
    // `Cast { target_ty: primitive }`, and the Cast-aware short-circuit
    // at the top of this fn handles them. With cross-category visibility
    // (rust.rs now passes `&extras` per category) the lowerer reaches
    // every LC-method callee; the AC::Base shim methods that aren't in
    // any LC don't have primitive params (Hash<Str, Untyped> only — see
    // `dispatch.rs::controller_shim_method_param_ty`), so the gap the
    // fallback used to cover is empty.

    // Family 3 + 3b — primitive → Value and Hash literal → Value(Object).
    // Two paths:
    //
    // 1. **Lowerer-inserted Cast** (common path for Send arg
    //    positions). The `lower::ty_coerce_insertion::
    //    needs_primitive_to_value` and `needs_hash_literal_to_value`
    //    lowerers wrap eligible args in `Cast { target_ty: Untyped |
    //    Class(ParamValue) | Record }`.
    //
    // 2. **Emit-time caller fallback** (for sites the lowerer can't
    //    see, like `index.rs::fetch` peephole calling this fn with an
    //    explicit `param_ty` derived from the recv's Hash value-Ty —
    //    `Hash#fetch` isn't an LC method so the lowerer's registry
    //    doesn't reach it). Gated on `!Cast` so the lowerer path
    //    stays authoritative for Send arg sites.
    //
    // Ivar args with non-Copy fields need `.clone()` first because
    // `Value::from` takes by value and would move out of `&self`
    // (E0507). Integer/Float/Bool fields are Copy so skip the clone.
    // `Untyped` / Record stay Value. Heterogeneous unions such as
    // `turbo_stream_from`'s `String | Array[untyped]` also render as
    // `serde_json::Value` — wrap scalar args here, rust-only. Do not
    // lift that to the shared lowerer: other targets do not box those
    // unions as Value, and treating them as value-shaped Casts broke
    // ActionController emit (crystal Int32/Int64, go/kotlin/csharp
    // index types, python `.tr` on str).
    let value_target = |ty: &Ty| -> bool {
        super::super::super::ty::rust_value_shaped(ty)
    };
    // A Hash literal already widened to a Hash (the lowered
    // `record.slice`) nests as an Object when the slot is a Value.
    if let ExprNode::Cast { value, target_ty: Ty::Hash { .. } } = &*arg.node {
        if let (ExprNode::Hash { entries, .. }, true) = (&*value.node, value_target(param_ty)) {
            return hash_literal_into_value(entries);
        }
    }
    if let ExprNode::Cast { value, target_ty } = &*arg.node {
        if value_target(target_ty) {
            // Family 3b — Hash literal → Value::Object.
            if let ExprNode::Hash { entries, .. } = &*value.node {
                return hash_literal_into_value(entries);
            }
            // Family 3 — primitive → Value.
            if let Some(time) = time_into_value(value) {
                return time;
            }
            let inner_ty_peeled = value.ty.as_ref().map(peel_nil);
            if matches!(
                inner_ty_peeled,
                Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool)
            ) {
                let inner_raw = emit_expr(value);
                let needs_clone = matches!(&*value.node, ExprNode::Ivar { .. })
                    && !matches!(inner_ty_peeled, Some(Ty::Int | Ty::Float | Ty::Bool));
                if needs_clone {
                    return format!("serde_json::Value::from({inner_raw}.clone())");
                }
                return format!("serde_json::Value::from({inner_raw})");
            }
        }
    }
    // Non-Cast fallback for the emit-time caller path (Hash#fetch and
    // similar peepholes that call `coerce_arg_for_param_ty` with an
    // explicit `param_ty` the lowerer didn't see).
    if !matches!(&*arg.node, ExprNode::Cast { .. }) && value_target(param_ty) {
        if let ExprNode::Hash { entries, .. } = &*arg.node {
            return hash_literal_into_value(entries);
        }
        if let Some(time) = time_into_value(arg) {
            return time;
        }
        if matches!(
            arg_ty_peeled,
            Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool)
        ) || matches!(
            &*arg.node,
            ExprNode::Lit {
                value: Literal::Str { .. }
                    | Literal::Sym { .. }
                    | Literal::Int { .. }
                    | Literal::Float { .. }
                    | Literal::Bool { .. }
            } | ExprNode::StringInterp { .. }
        ) {
            let needs_clone = matches!(&*arg.node, ExprNode::Ivar { .. })
                && !matches!(arg_ty_peeled, Some(Ty::Int | Ty::Float | Ty::Bool));
            if needs_clone {
                return format!("serde_json::Value::from({raw}.clone())");
            }
            return format!("serde_json::Value::from({raw})");
        }
    }

    // Family 5: owned-T clone for Ivar / Var / SelfRef args feeding
    // a callee param that takes owned non-Copy T. The caller's
    // `self.X` Ivar read produces `self.X` (a borrowed-from-&self
    // place expression); passing it to an owned param trips E0507
    // ("cannot move out of self.X which is behind a shared
    // reference") or E0308 ("expected T, found &T"). Inserting
    // `.clone()` materializes the owned value.
    //
    // The model lowerer's `broadcasts_to` expansion (`Articles::
    // article(self, None, None)`) and the controller lowerer's
    // `<Model>::from_params(self.params)` rewrite are the canonical
    // cases: the first hands owned Class param; the second hands an
    // owned HashMap param.
    //
    // Conservative firing: only when arg.ty matches the callee's
    // param ty exactly AND arg is SelfRef/Var/Ivar (always emit as a
    // borrowed-place reference, not owned). Send arms returning
    // owned T don't need a clone; Const-typed args don't either.
    //
    // **Why this stays in rust and isn't lifted to the
    // `ty_coerce_insertion` lowerer.** The decision is Rust-borrow-
    // semantics-specific: it's not "the IR's typing is wrong" but
    // "the Rust callsite would consume from a borrowed place." TS,
    // Crystal, Go, Spinel all auto-clone or auto-borrow without a
    // separate annotation. Lifting this to the lowerer would either
    // produce a Cast that all targets need to learn to ignore, or a
    // rust-specific IR annotation that doesn't fit the cross-target
    // Cast shape. Same reasoning as #22's "bits 32–63 are rust-
    // local" partition.
    if matches!(
        param_ty,
        Ty::Class { .. } | Ty::Hash { .. } | Ty::Array { .. }
    ) {
        // Outer-shape match suffices — Hash<Str, Untyped> and
        // Hash<Str, Class(ParamValue)> both render as
        // `HashMap<String, serde_json::Value>` at Rust level (since
        // ParamValue is a type alias for Value). A strict
        // `arg.ty == param_ty` gate misses these. Class arms still
        // demand id-equality so `Article` ≠ `Comment` for the clone.
        let outer_shape_matches = match (param_ty, arg.ty.as_ref()) {
            (Ty::Class { id: pid, .. }, Some(Ty::Class { id: aid, .. })) => pid == aid,
            (Ty::Hash { .. }, Some(Ty::Hash { .. })) => true,
            (Ty::Array { .. }, Some(Ty::Array { .. })) => true,
            _ => false,
        };
        let arg_is_borrowed_place = matches!(
            &*arg.node,
            ExprNode::SelfRef | ExprNode::Var { .. } | ExprNode::Ivar { .. }
        );
        if outer_shape_matches && arg_is_borrowed_place {
            return format!("{raw}.clone()");
        }
    }

    // Family 7: Option<Str> → &str. When the arg's body-typer Ty is
    // `Option<String>` (i.e., `Union<Str, Nil>`) and the callee takes
    // `&str`, the bare emit is the Option-shape — `as_deref()` borrows
    // the inner `&str` and `unwrap_or("")` substitutes the empty string
    // for None. Closes the layouts.rs::application path where
    // `html_escape(content_for_get(:title))` reaches a callee declared
    // `(&str) -> String` with a `Option<String>` source.
    //
    // **Why this stays in rust and isn't lifted to the
    // `ty_coerce_insertion` lowerer.** The decision composes two
    // rust-local shape choices: (1) rust emits `Ty::Str` params as
    // `&str` (not `String`) — a target-rendering decision — and (2)
    // `Option<String>::as_deref().unwrap_or("")` is the idiomatic
    // Rust deref shape; Crystal's `option || ""`, TS's `?? ""`, Go's
    // zero-value fallback all spell this differently. Lifting to the
    // lowerer would either over-fire on targets that handle nilable
    // strings natively or produce a target-specific Cast — neither
    // pulls its weight vs. keeping the decision rust-local.
    if param_ty.is_stringish()
        && !super::super::has_str_coercion(arg)
        && !super::is_array_index_read(arg)
        && !var_binding_is_non_option(arg)
        && matches!(arg.ty.as_ref(), Some(t) if is_option_ty(t))
        && matches!(
            arg.ty.as_ref().and_then(|t| {
                if let Ty::Union { variants } = t {
                    variants.iter().find(|v| !matches!(v, Ty::Nil)).cloned()
                } else {
                    None
                }
            }),
            Some(Ty::Str | Ty::Sym)
        )
    {
        return format!("{raw}.as_deref().unwrap_or(\"\")");
    }

    if param_ty.is_stringish() && !super::super::has_str_coercion(arg) {
        // Value-shaped arg → `&str` param. Hash#fetch / Hash#[] on an
        // untyped bag (the `form_with` → `method_override_input`
        // path) and Value-typed locals/ivars need `.as_str()`. Do
        // not key off Untyped Sends in general: a Const-recv helper
        // like `Views::Articles::article(...)` can be typed Untyped
        // in IR while rust emits `String`/`&str`, and wrapping that
        // in `.as_str().unwrap()` is E0599.
        if json_value_arg_for_string_param(arg) {
            return json_value_as_str(raw, arg);
        }
        // Peek through `Cast` wrappers — the model lowerer wraps row
        // accessors in `Cast { Send(row.col), col_ty }` to bridge
        // Crystal's nilable row holder, but rust's row class is
        // already non-Nilable so the Cast renders as the bare inner
        // call. The "is this owned String?" check has to see the
        // inner node to fire.
        let owned_producing_node = |n: &ExprNode| {
            matches!(
                n,
                ExprNode::Var { .. }
                    | ExprNode::Send { .. }
                    | ExprNode::Ivar { .. }
                    | ExprNode::BoolOp { .. }
                    | ExprNode::StringInterp { .. }
            )
        };
        // Conservative "If-expr produces owned String": both branches
        // are owned-producers AND the If's body-typer Ty is Str.
        // Closes `html_escape(if persisted? { article_path(id) } else
        // { articles_path() })` where each branch is a Send returning
        // owned String. Borrowing the whole If-expr (`&(if ... {} else
        // {})`) makes Rust coerce both arms' Strings into a shared
        // `&str` via Deref.
        let if_owned_producing = if let ExprNode::If { then_branch, else_branch, .. } = &*arg.node {
            owned_producing_node(&*then_branch.node)
                && owned_producing_node(&*else_branch.node)
        } else {
            false
        };
        let arg_is_owned = matches!(arg_ty_peeled, Some(Ty::Str | Ty::Sym))
            && (owned_producing_node(&*arg.node)
                || if_owned_producing
                || matches!(
                    &*arg.node,
                    ExprNode::Cast { value, .. } if owned_producing_node(&*value.node)
                ));
        if arg_is_owned {
            return format!("&({raw})");
        }
    }

    raw
}

/// When a Cast's source type renders as `serde_json::Value` at the
/// rust emit (a non-Nilable multi-variant Union — `Union<i64,
/// String, …>` from the lowerer-synthesized column-union types), and
/// the target type is a primitive (`Str`/`Sym`/`Int`/`Float`/`Bool`),
/// emit the corresponding `.as_X().unwrap()` coercion.
pub(crate) fn cast_via_value_for_union(value: &Expr, target_ty: &crate::ty::Ty) -> Option<String> {
    use crate::ty::Ty;
    let value_shaped = match value.ty.as_ref() {
        Some(Ty::Union { variants }) => {
            let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
            !(variants.len() == 2 && has_nil)
        }
        _ => false,
    };
    if !value_shaped {
        return None;
    }
    let raw = emit_expr(value);
    if let Some(inner) = peel_nilable(target_ty) {
        return match inner {
            Ty::Str | Ty::Sym => Some(format!("({raw}).as_str().map(|s| s.to_string())")),
            Ty::Int => Some(format!("({raw}).as_i64()")),
            Ty::Float => Some(format!("({raw}).as_f64()")),
            Ty::Bool => Some(format!("({raw}).as_bool()")),
            // A `serde_json::Value` cannot BE a `DateTime<Utc>`, so the
            // only honest reading is the stored text form — which is
            // exactly what `parse_db_time` accepts, and it already
            // returns the Option this arm wants.
            Ty::Time => {
                Some(format!("({raw}).as_str().and_then(crate::rh_datetime::parse_db_time)"))
            }
            _ => None,
        };
    }
    match target_ty {
        Ty::Str | Ty::Sym => Some(format!("({raw}).as_str().unwrap().to_string()")),
        Ty::Int => Some(format!("({raw}).as_i64().unwrap()")),
        Ty::Float => Some(format!("({raw}).as_f64().unwrap()")),
        Ty::Bool => Some(format!("({raw}).as_bool().unwrap()")),
        // Same reading as the nilable arm, unwrapped. Reached from the
        // Hash-shaped `update`'s temporal normalize
        // (`format_db_time(Cast(attrs[:created_at], Time))`), which is
        // nil-guarded at the IR level — a `Value` that is neither a
        // string nor db-form time is a caller error, and a checked cast
        // is allowed to say so.
        Ty::Time => Some(format!(
            "crate::rh_datetime::parse_db_time(({raw}).as_str().unwrap()).unwrap()"
        )),
        _ => None,
    }
}

/// Family 3b: a Hash literal into a `serde_json::Value` slot, as a
/// `Value::Object` built entry by entry. Not the literal as a `HashMap`
/// first: its values need not share a type (`slice(:id, :created_at)`
/// is an Integer beside a Time), and a `HashMap::from` of them cannot
/// unify before any per-value conversion runs.
fn hash_literal_into_value(entries: &[(Expr, Expr)]) -> String {
    if entries.is_empty() {
        return "serde_json::Value::Object(serde_json::Map::new())".to_string();
    }
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            let v_s = time_into_value(v).unwrap_or_else(|| format!("serde_json::Value::from({})", emit_expr(v)));
            format!("(({}).to_string(), {v_s})", emit_expr(k))
        })
        .collect();
    format!("serde_json::Value::Object(serde_json::Map::from_iter([{}]))", pairs.join(", "))
}

/// A `Time` into a `serde_json::Value` slot. A Value cannot hold a
/// `DateTime<Utc>`; it holds the stored text form, as `attributes` and
/// `get_index` do, which is what `parse_db_time` reads back. A nil time
/// is JSON null.
pub(crate) fn time_into_value(arg: &Expr) -> Option<String> {
    use crate::ty::Ty;
    let ty = arg.ty.as_ref()?;
    if !matches!(peel_nil(ty), Ty::Time) {
        return None;
    }
    let raw = emit_expr(arg);
    Some(if is_option_ty(ty) {
        format!("serde_json::Value::from(({raw}).map(crate::rh_datetime::format_db_time))")
    } else {
        format!("serde_json::Value::from(crate::rh_datetime::format_db_time({raw}))")
    })
}

/// `Union{[T, Nil]}` → `T`. A nullable column's field/param carries
/// that shape, and the serde_json accessors already return exactly the
/// Option we want — `as_str()` is `Option<&str>` — so the nullable
/// coercion is the non-nullable one minus its `.unwrap()`.
fn peel_nilable(ty: &crate::ty::Ty) -> Option<&crate::ty::Ty> {
    use crate::ty::Ty;
    let Ty::Union { variants } = ty else { return None };
    if variants.len() != 2 {
        return None;
    }
    if !variants.iter().any(|v| matches!(v, Ty::Nil)) {
        return None;
    }
    variants.iter().find(|v| !matches!(v, Ty::Nil))
}


/// Field-position coercion: variant of `coerce_arg_for_param_ty` for
/// the constructor's `let <field> = <value>` rewrite. Two differences
/// from param-position coercion:
///
/// 1. **String fields want owned `String`**, not `&str`. After the
///    Value→`&str` `.as_str().unwrap()`, append `.to_string()`.
/// 2. **Union-containing-Untyped triggers Value-narrowing too** —
///    `BoolOp::Or` of `hash[k] || 0` types as `Union<Union<Untyped,
///    Nil>, Int>`, neither peel_nil nor a flat Union-of-Untyped+Nil.
///    Recursively probe for Untyped via `ty_contains_untyped`.
pub(crate) fn coerce_arg_for_field_ty(arg: &Expr, field_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let raw = emit_expr(arg);
    let value_shaped = arg.ty.as_ref().map(ty_contains_untyped).unwrap_or(false);
    if value_shaped {
        let coercion = match peel_nilable(field_ty).unwrap_or(field_ty) {
            inner if peel_nilable(field_ty).is_some() => match inner {
                Ty::Str | Ty::Sym => Some("as_str().map(|s| s.to_string())"),
                Ty::Int => Some("as_i64()"),
                Ty::Float => Some("as_f64()"),
                Ty::Bool => Some("as_bool()"),
                // A `serde_json::Value` can only carry a time as its
                // stored TEXT form, which is exactly what
                // `parse_db_time` reads — and it already returns the
                // Option this arm wants.
                Ty::Time => Some("as_str().and_then(crate::rh_datetime::parse_db_time)"),
                _ => None,
            },
            Ty::Str | Ty::Sym => Some("as_str().unwrap().to_string()"),
            Ty::Int => Some("as_i64().unwrap()"),
            Ty::Float => Some("as_f64().unwrap()"),
            Ty::Bool => Some("as_bool().unwrap()"),
            // Same reading, unwrapped — reached from the Hash-shaped
            // `update`'s temporal normalize, which is nil-guarded at the
            // IR level.
            Ty::Time => Some("as_str().and_then(crate::rh_datetime::parse_db_time).unwrap()"),
            _ => None,
        };
        if let Some(c) = coercion {
            return format!("({raw}).{c}");
        }
    }
    // Value → HashMap<String, Value> narrowing. Covers Untyped /
    // Record / Class(Roundhouse::ParamValue) sources — all of which
    // rust renders as `serde_json::Value` — when the target field
    // shape is the conventional `HashMap<String, ParamValue/Untyped>`.
    // The `synth_from_raw` lowerer wraps the post-`is_a?(Hash)` Var
    // in a Cast for this purpose, so a bare `raw_sub` site emits as
    // `raw_sub.as_object().cloned().unwrap_or_default().into_iter().
    // collect::<HashMap<_, _>>()`. Restricted to String-keyed maps
    // with a Value-shaped value type so the Map → HashMap conversion's
    // element types unify trivially.
    //
    // Source shape detection inspects both `arg.ty` (the body-typer's
    // post-narrowing view: e.g. `Hash<Untyped, Untyped>` for a
    // `raw_sub.is_a?(Hash)`-guarded Var read) AND the declared local
    // var type via `local_var_ty` (the storage shape: Value /
    // ParamValue). Either being Value-shaped triggers the conversion;
    // a true `HashMap<Value, Value>` source would need neither, so a
    // false positive here is rare and benign.
    let is_value_shaped = |t: &Ty| -> bool {
        // Peel an outer `Option<...>` (Union<T, Nil>) before checking,
        // since the body-typer often types a `params.fetch(k, default)`
        // result as `Union<ParamValue, Nil>` even when the actual Rust
        // emit produces an `unwrap_or(...)`-flattened `Value`.
        let inner = super::super::util::peel_nil(t);
        matches!(inner, Ty::Untyped | Ty::Record { .. })
            || matches!(
                inner,
                Ty::Class { id, .. } if id.0.as_str() == "Roundhouse::ParamValue"
            )
    };
    let arg_renders_as_value = arg.ty.as_ref().map(is_value_shaped).unwrap_or(false)
        || {
            // Walk through a `.clone()` wrap to reach the underlying
            // Var name, then look up its declared local type.
            let inner: &Expr = match &*arg.node {
                ExprNode::Send { recv: Some(r), method, args: m_args, .. }
                    if method.as_str() == "clone" && m_args.is_empty() =>
                {
                    r
                }
                _ => arg,
            };
            match &*inner.node {
                ExprNode::Var { name, .. } => {
                    super::super::local_var_ty(name.as_str())
                        .as_ref()
                        .map(is_value_shaped)
                        .unwrap_or(false)
                }
                _ => false,
            }
        };
    if arg_renders_as_value {
        if let Ty::Hash { key, value } = field_ty {
            if key.is_stringish() {
                let value_is_value_shaped = matches!(**value, Ty::Untyped | Ty::Record { .. })
                    || matches!(
                        &**value,
                        Ty::Class { id, .. } if id.0.as_str() == "Roundhouse::ParamValue"
                    );
                if value_is_value_shaped {
                    return format!(
                        "{raw}.as_object().cloned().unwrap_or_default().into_iter().collect::<std::collections::HashMap<String, serde_json::Value>>()"
                    );
                }
            }
        }
    }
    raw
}

/// True when `arg` rust-emits as `serde_json::Value` and is being
/// passed to a stringish (`&str`) param. Hash#[] / Hash#fetch on an
/// untyped bag, plus Value-typed locals/ivars. Class-method Sends are
/// excluded even when IR types them Untyped — their rust emit is the
/// callee's concrete return (`String`, a model, …).
fn json_value_arg_for_string_param(arg: &Expr) -> bool {
    let inner = if let ExprNode::Cast { value, .. } = &*arg.node {
        value
    } else {
        arg
    };
    let value_shaped = |t: &crate::ty::Ty| {
        let peeled = peel_nil(t);
        super::super::super::ty::rust_value_shaped(t)
            || super::super::super::ty::rust_value_shaped(peeled)
            || ty_contains_untyped(t)
    };
    match &*inner.node {
        ExprNode::Var { .. } | ExprNode::Ivar { .. } => {
            // `Union<Untyped, Nil>` rust-emits `Option<Value>`. Peel
            // would call that a Value and emit `.as_str()` on Option.
            inner.ty.as_ref().is_some_and(|t| !is_option_ty(t) && value_shaped(t))
        }
        ExprNode::Send { method, recv, .. } => {
            if !matches!(method.as_str(), "[]" | "fetch" | "get") {
                return false;
            }
            if matches!(
                recv.as_ref().map(|r| &*r.node),
                Some(ExprNode::Const { .. })
            ) {
                return false;
            }
            // Hash#[] on an untyped bag is often typed `Untyped | Nil`
            // (Ruby nil-on-miss) while the rust peephole emits a
            // `Value` (`get().cloned().unwrap_or(Null)`), not Option.
            inner.ty.as_ref().is_some_and(value_shaped)
        }
        _ => false,
    }
}

/// `.as_str()` needs an owned `Value`. HashMap#get rust-emits
/// `Option<&Value>`; Hash#fetch(k, nil) peepholes to the same
/// `get().cloned()` Option. Materialize Null-on-miss first.
fn json_value_as_str(raw: String, arg: &Expr) -> String {
    let inner = if let ExprNode::Cast { value, .. } = &*arg.node {
        value
    } else {
        arg
    };
    let owned = if let ExprNode::Send { method, recv, args, .. } = &*inner.node {
        let hash_recv = matches!(
            recv.as_ref().and_then(|r| r.ty.as_ref()).map(peel_nil),
            Some(crate::ty::Ty::Hash { .. })
        );
        let fetch_nil = method.as_str() == "fetch"
            && args.len() == 2
            && matches!(&*args[1].node, ExprNode::Lit { value: Literal::Nil });
        if hash_recv && (method.as_str() == "get" || fetch_nil) {
            format!("{raw}.cloned().unwrap_or(serde_json::Value::Null)")
        } else {
            raw
        }
    } else {
        raw
    };
    format!("({owned}).as_str().unwrap()")
}

/// serde_json accessor that yields `Option<T>` directly — the nullable
/// twin of `util::value_narrowing_coercion`, which appends `.unwrap()`.
fn value_narrowing_coercion_opt(inner: &crate::ty::Ty) -> Option<&'static str> {
    use crate::ty::Ty;
    match inner {
        Ty::Str | Ty::Sym => Some("as_str().map(|s| s.to_string())"),
        Ty::Int => Some("as_i64()"),
        Ty::Float => Some("as_f64()"),
        Ty::Bool => Some("as_bool()"),
        _ => None,
    }
}

/// Is `arg` a Var whose emitted BINDING is non-Option, even though the
/// body-typer's Ty for the read says `T | Nil`?
///
/// The binding's recorded type is the one `assign.rs` wrote when it
/// emitted the `let`, and it already accounts for rust's own rendering
/// choices — notably that an Array index read panics rather than
/// answering an Option. When the two disagree, the binding wins: it
/// describes the value that actually exists at this call site.
fn var_binding_is_non_option(arg: &Expr) -> bool {
    let ExprNode::Var { name, .. } = &*arg.node else {
        return false;
    };
    super::super::local_var_ty(name.as_str())
        .map(|t| !is_option_ty(&t))
        .unwrap_or(false)
}
