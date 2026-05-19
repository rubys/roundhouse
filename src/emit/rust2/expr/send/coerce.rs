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

use crate::expr::{Expr, ExprNode};

use super::super::util::{is_option_ty, peel_nil, ty_contains_untyped, value_narrowing_coercion};
use super::super::{arg_hash_var_local_ty, class_method_param_ty, emit_expr};

/// Apply callee-back-propagation coercion for a single arg in a
/// class-/instance-method call where the callee is in
/// `CLASS_METHOD_PARAM_TYS`. Defers to `coerce_arg_for_param_ty`.
///
/// Two-step lookup: current-class first (sibling method on the same
/// LC), then the `controller_shim_method_param_ty` table for the
/// per-controller AC::Base shim methods (`redirect_to`, `render_with`)
/// whose signatures live as hand-coded text in `rust2.rs::emit` and
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
/// 4. **String → &str (Borrow)**: callee `Str|Sym` (rust2 emits
///    `&str` for these param positions) with arg from a non-literal
///    String-producing source (Var/Send/Ivar) → `&(raw)`.
pub(crate) fn coerce_arg_for_param_ty(arg: &Expr, param_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let raw = emit_expr(arg);
    let arg_ty_peeled = arg.ty.as_ref().map(peel_nil);

    // Family 6: T → Option<T> Some-wrap. When the callee param is
    // `Option<U>` (rust2's emit shape for RBS-declared `U?` / `T |
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
        let inner = peel_nil(param_ty);
        let owned_producing = matches!(
            &*arg.node,
            ExprNode::Var { .. } | ExprNode::Send { .. } | ExprNode::Ivar { .. }
        );
        if owned_producing
            && arg.ty.as_ref() == Some(inner)
            && !matches!(inner, Ty::Untyped)
        {
            return format!("Some({raw})");
        }
    }

    if let Ty::Hash { value: pv, .. } = param_ty {
        if matches!(pv.as_ref(), Ty::Untyped) {
            // Var arg with a local Hash type that doesn't match —
            // wrap with the K/V-coercing conversion.
            if let Some((_lk, _lv)) = arg_hash_var_local_ty(arg) {
                return format!(
                    "{raw}.into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v))).collect::<std::collections::HashMap<String, serde_json::Value>>()"
                );
            }
            // Hash-literal arg: HashMap::from([…]) typically infers
            // `HashMap<&str, T>` from the first entry, which won't
            // unify with the callee's `HashMap<String, Value>`. Apply
            // the same transform unconditionally.
            if matches!(&*arg.node, ExprNode::Hash { .. }) {
                return format!(
                    "{raw}.into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v))).collect::<std::collections::HashMap<String, serde_json::Value>>()"
                );
            }
        }
    }

    if matches!(arg_ty_peeled, Some(Ty::Untyped)) {
        if let Some(coerce) = value_narrowing_coercion(param_ty) {
            return format!("({raw}).{coerce}");
        }
    }

    // Family 3: primitive → Value. For Ivar reads of non-Copy fields,
    // `Value::from` takes by value and would move out of `&self`. Clone
    // first to materialize the owned value.
    if matches!(param_ty, Ty::Untyped)
        && matches!(
            arg_ty_peeled,
            Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool)
        )
    {
        let needs_clone = matches!(&*arg.node, ExprNode::Ivar { .. })
            && !matches!(arg_ty_peeled, Some(Ty::Int | Ty::Float | Ty::Bool));
        if needs_clone {
            return format!("serde_json::Value::from({raw}.clone())");
        }
        return format!("serde_json::Value::from({raw})");
    }

    // Family 3b: Hash literal → Value (Object). When callee/storage
    // expects `serde_json::Value` (Ty::Untyped) or a Record (which
    // rust2's ty.rs renders as Value too) and the arg is an inline
    // Hash literal (`{ name: "x", path: "/y" }`), the bare emit is
    // `HashMap::from([…])` which doesn't unify with Value. Reshape:
    // convert keys to String, wrap each value in Value::from, then
    // build a Value::Object. The Importmap pins literal is the
    // canonical case (Vec<Record>).
    if matches!(param_ty, Ty::Untyped | Ty::Record { .. })
        && matches!(&*arg.node, ExprNode::Hash { .. })
    {
        return format!(
            "serde_json::Value::Object({raw}.into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v))).collect())"
        );
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

    if matches!(param_ty, Ty::Str | Ty::Sym) && arg.str_coercion.is_none() {
        // Peek through `Cast` wrappers — the model lowerer wraps row
        // accessors in `Cast { Send(row.col), col_ty }` to bridge
        // Crystal's nilable row holder, but rust2's row class is
        // already non-Nilable so the Cast renders as the bare inner
        // call. The "is this owned String?" check has to see the
        // inner node to fire.
        let owned_producing_node = |n: &ExprNode| {
            matches!(
                n,
                ExprNode::Var { .. } | ExprNode::Send { .. } | ExprNode::Ivar { .. }
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
/// rust2 emit (a non-Nilable multi-variant Union — `Union<i64,
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
    match target_ty {
        Ty::Str | Ty::Sym => Some(format!("({raw}).as_str().unwrap().to_string()")),
        Ty::Int => Some(format!("({raw}).as_i64().unwrap()")),
        Ty::Float => Some(format!("({raw}).as_f64().unwrap()")),
        Ty::Bool => Some(format!("({raw}).as_bool().unwrap()")),
        _ => None,
    }
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
        let coercion = match field_ty {
            Ty::Str | Ty::Sym => Some("as_str().unwrap().to_string()"),
            Ty::Int => Some("as_i64().unwrap()"),
            Ty::Float => Some("as_f64().unwrap()"),
            Ty::Bool => Some("as_bool().unwrap()"),
            _ => None,
        };
        if let Some(c) = coercion {
            return format!("({raw}).{c}");
        }
    }
    raw
}
