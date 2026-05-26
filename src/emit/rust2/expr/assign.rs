//! Assignment emit — Var, Ivar, Attr, Index LValues. Handles the
//! interaction with the local-var declared-type cache, the auto-mut
//! / declared-var sets, and the constructor / module-singleton
//! mode-switches that change what an `@field = value` lowers to.

use crate::expr::{Expr, ExprNode, LValue, Literal};

use super::util::{coerce_to_value, is_builtin_container_class, is_option_ty};
use super::{
    current_return_ty, declare_var, emit_expr, in_constructor, in_module_singleton,
    is_declared_var, is_mut_var, ivar_field_ty,
    local_var_ty, mark_local_var_ty, module_singleton_slot_name, param_ty,
    record_back_propagated_hash,
};

pub(super) fn emit_assign(target: &LValue, value: &Expr) -> String {
    let rhs = emit_expr(value);
    match target {
        LValue::Var { name, .. } => {
            let name_str = name.as_str().to_string();
            // Track local-var declared type for the narrowing-aware
            // Var read. Only records on first assignment — subsequent
            // rebinds leave the recorded declared type alone (Rust's
            // `mut` binding type is fixed).
            //
            // For empty-HashMap inits in Hash-returning functions, the
            // back-propagated `Hash<K, V>` ty takes precedence over the
            // body-typer's `Hash<Untyped, Untyped>` view — subsequent
            // `.insert` emits use this to coerce args to the right K/V.
            if local_var_ty(&name_str).is_none() {
                let back_propagated = empty_hash_return_ty(value)
                    .or_else(|| none_init_option_return_ty(value));
                if back_propagated.is_some() {
                    record_back_propagated_hash(name_str.clone());
                }
                let ty = back_propagated.or_else(|| value.ty.clone());
                if let Some(t) = ty {
                    mark_local_var_ty(&name_str, t);
                }
            }
            if is_declared_var(&name_str) {
                // Some-wrap when the binding was declared `Option<T>`
                // and the new RHS is plain `T`. Without this,
                // `result = instance.clone()` after `result = None`
                // fails E0308. Catches the lowerer-synthesized
                // `result = instance; ...; result` accumulator pattern
                // in `_adapter_find_by_id` / `find` and friends.
                let rhs_wrapped = some_wrap_for_assign(&name_str, value, &rhs);
                return format!("{name_str} = {rhs_wrapped}");
            }
            let needs_mut = is_mut_var(&name_str);
            // Type-annotate empty HashMap literals when the enclosing
            // function returns a Hash<K, V> (or Option<Hash<K, V>>).
            // Without an annotation, Rust infers params' type from the
            // FIRST `.insert(k, v)` — often `HashMap<&str, &str>` from
            // borrowed source data, which mismatches the function's
            // declared `Hash<String, String>?` return.
            let annot = empty_hash_return_annotation(value);
            declare_var(name_str.clone());
            if needs_mut {
                format!("let mut {name_str}{annot} = {rhs}")
            } else {
                format!("let {name_str}{annot} = {rhs}")
            }
        }
        LValue::Ivar { name } => {
            let rhs_coerced = maybe_to_string_coercion(name.as_str(), value, &rhs);
            if in_module_singleton() {
                // Module-singleton ivar write — route through the
                // static Mutex slot. Always Some-wraps so the slot
                // stays `Option<T>` regardless of T's nullability.
                let slot = module_singleton_slot_name(name.as_str());
                return format!("*{slot}.lock().unwrap() = Some({rhs_coerced})");
            }
            if in_constructor() {
                // Annotate the let with the field's declared type so
                // the closing `Self { f1, f2, ... }` literal sees
                // matching types. Without the annotation, a `let mut
                // body = ""` declared as `&str` collides with the
                // `String`-typed field at the Self literal site.
                let annot = field_let_annotation(name.as_str());
                return format!("let mut {name}{annot} = {rhs_coerced}");
            }
            format!("self.{name} = {rhs_coerced}")
        }
        LValue::Attr { recv, name } => {
            // `self.x = ...` inside a module-singleton class method
            // refers to the class itself, not an instance — route
            // through the static slot.
            if in_module_singleton() && matches!(&*recv.node, ExprNode::SelfRef) {
                let slot = module_singleton_slot_name(name.as_str());
                return format!("*{slot}.lock().unwrap() = Some({rhs})");
            }
            format!("{}.{name} = {rhs}", emit_expr(recv))
        }
        LValue::Index { recv, index } => {
            // `recv[k] = v` on a Flash / Session struct dispatches to
            // the hand-written `.set(key, value)` method (no IndexMut
            // impl; the runtime/rust/flash.rs etc. surface explicit
            // setters).
            if let Some(crate::ty::Ty::Class { id, .. }) = recv.ty.as_ref() {
                let cls = id.0.as_str();
                if matches!(cls, "Flash" | "ActionDispatch::Flash") {
                    // Flash::set takes `Option<String>` (per
                    // runtime/rust/flash.rs). Wrap a non-Option-shaped
                    // rhs in `Some(...)` so the narrowed-Var emit
                    // reaches a typed slot.
                    let rhs_is_option = matches!(
                        value.ty.as_ref(),
                        Some(crate::ty::Ty::Union { variants })
                            if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                    );
                    let wrapped = if rhs_is_option {
                        rhs.clone()
                    } else {
                        format!("Some({rhs})")
                    };
                    return format!(
                        "{}.set({}, {wrapped})",
                        emit_expr(recv),
                        emit_expr(index),
                    );
                }
                if matches!(cls, "Session" | "ActionDispatch::Session") {
                    return format!(
                        "{}.set({}, {rhs})",
                        emit_expr(recv),
                        emit_expr(index),
                    );
                }
                // Other Ty::Class receivers route through `set_index`
                // (per the operator-method rewrite in `sanitize_ident`).
                // Wrap String RHS with `serde_json::Value::from`
                // because `def []=` is declared `(Symbol, untyped) ->
                // untyped`, which renders the value param as
                // `serde_json::Value`.
                if !is_builtin_container_class(cls) {
                    let coerced_rhs = coerce_to_value(value, &rhs);
                    return format!(
                        "{}.set_index({}, {coerced_rhs})",
                        emit_expr(recv),
                        emit_expr(index),
                    );
                }
            }
            // HashMap doesn't implement IndexMut — `recv[k] = v`
            // requires `.insert(k, v)`. Wrap in `{ ...; }` so the
            // assignment evaluates to `()` (insert returns Option<V>
            // which rustc rejects in no-else if-statement contexts).
            if matches!(recv.ty.as_ref(), Some(crate::ty::Ty::Hash { .. })) {
                return format!(
                    "{{ {}.insert({}, {rhs}); }}",
                    emit_expr(recv),
                    emit_expr(index),
                );
            }
            format!("{}[{}] = {rhs}", emit_expr(recv), emit_expr(index))
        }
    }
}

/// Return `Hash<K, V>` when `value` is the empty `{}` literal AND
/// the enclosing function's declared return type is `Hash<K, V>` (or
/// `Option<Hash<K, V>>`).
fn empty_hash_return_ty(value: &Expr) -> Option<crate::ty::Ty> {
    let is_empty_hash = matches!(
        &*value.node,
        ExprNode::Hash { entries, .. } if entries.is_empty()
    );
    if !is_empty_hash {
        return None;
    }
    match current_return_ty() {
        Some(crate::ty::Ty::Hash { key, value }) => Some(crate::ty::Ty::Hash { key, value }),
        Some(crate::ty::Ty::Union { variants }) => variants
            .into_iter()
            .find(|v| matches!(v, crate::ty::Ty::Hash { .. })),
        _ => None,
    }
}

/// `Option<T>` (or the unioned `Union<T, Nil>`) when `value` is the
/// `nil` literal AND the enclosing function returns `Option<T>`.
fn none_init_option_return_ty(value: &Expr) -> Option<crate::ty::Ty> {
    let is_nil_lit = matches!(
        &*value.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    if !is_nil_lit {
        return None;
    }
    match current_return_ty() {
        Some(t) if is_option_ty(&t) => Some(t),
        _ => None,
    }
}

fn empty_hash_return_annotation(value: &Expr) -> String {
    match empty_hash_return_ty(value) {
        Some(crate::ty::Ty::Hash { key, value }) => format!(
            ": std::collections::HashMap<{}, {}>",
            super::super::ty::rust_ty(&key),
            super::super::ty::rust_ty(&value),
        ),
        _ => match none_init_option_return_ty(value) {
            Some(t) => format!(": {}", super::super::ty::rust_ty(&t)),
            None => String::new(),
        },
    }
}

/// Wrap an RHS with `Some(...)` when the variable's recorded
/// `local_var_ty` is `Option<T>` and the RHS produces non-Option `T`.
fn some_wrap_for_assign(name: &str, value: &Expr, rhs: &str) -> String {
    let Some(declared) = local_var_ty(name) else {
        return rhs.to_string();
    };
    if !is_option_ty(&declared) {
        return rhs.to_string();
    }
    let rhs_is_option = value
        .ty
        .as_ref()
        .map(is_option_ty)
        .unwrap_or(false);
    if rhs_is_option {
        return rhs.to_string();
    }
    // `self` rhs in a `&self`/`&mut self` method: `Some(self)` would
    // produce `Option<&Self>`. The lowered `_adapter_save` shape
    // wants the owned `Option<Self>`, so clone.
    if matches!(&*value.node, ExprNode::SelfRef) {
        return format!("Some({rhs}.clone())");
    }
    format!("Some({rhs})")
}

/// Coerce RHS expressions to the declared field type when emit
/// produces a known-incompatible shape: `&str → String`, `T →
/// Option<T>`, and `Option<T> → T` (after a `.nil?` guard).
fn maybe_to_string_coercion(ivar_name: &str, value: &Expr, rhs: &str) -> String {
    let Some(field_ty) = ivar_field_ty(ivar_name) else {
        return rhs.to_string();
    };
    let (inner_field_ty, needs_some) = match &field_ty {
        crate::ty::Ty::Union { variants } if variants.len() == 2 => {
            let nil_idx = variants.iter().position(|v| matches!(v, crate::ty::Ty::Nil));
            match nil_idx {
                Some(0) => (variants[1].clone(), true),
                Some(1) => (variants[0].clone(), true),
                _ => (field_ty.clone(), false),
            }
        }
        _ => (field_ty.clone(), false),
    };
    // Authoritative RHS Ty: prefer the body-typer's `value.ty` (it
    // reflects flow-sensitive narrowing) over the RBS-declared param
    // ty, which the param-table fallback covers only when the
    // body-typer hasn't run.
    let effective_value_ty = match &*value.node {
        ExprNode::Var { name, .. } => value.ty.clone().or_else(|| param_ty(name.as_str())),
        _ => value.ty.clone(),
    };
    let rhs_is_option = matches!(
        effective_value_ty.as_ref(),
        Some(crate::ty::Ty::Union { variants }) if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
    );
    let str_color_handled = super::has_str_coercion(value);
    let coerced = if !str_color_handled
        && matches!(inner_field_ty, crate::ty::Ty::Str | crate::ty::Ty::Sym)
        && matches!(effective_value_ty.as_ref(), Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym))
    {
        format!("{rhs}.to_string()")
    } else if !needs_some && rhs_is_option {
        // Field is non-Option but RHS is Option-typed — unwrap. Only
        // safe after an `if x.is_none() return / skip` guard (the
        // Ruby `unless x.nil?` idiom).
        format!("{rhs}.unwrap()")
    } else {
        rhs.to_string()
    };
    if needs_some && !rhs_is_option && !matches!(effective_value_ty.as_ref(), Some(crate::ty::Ty::Nil)) {
        format!("Some({coerced})")
    } else {
        coerced
    }
}

/// In constructor mode, render the type annotation for the let
/// binding that backs the named ivar. Returns `: <Ty>` when the
/// field type is known, empty string otherwise.
fn field_let_annotation(ivar_name: &str) -> String {
    match ivar_field_ty(ivar_name) {
        Some(ty) => format!(": {}", super::super::ty::rust_ty(&ty)),
        None => String::new(),
    }
}

