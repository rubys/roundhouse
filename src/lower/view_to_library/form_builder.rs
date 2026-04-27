//! FormBuilder method dispatch — emit `<form>.<method>(args)` calls
//! with spinel-runtime-shape arg normalization.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

use crate::lower::view::FormBuilderMethod;

use super::{nil_lit, send, var_ref};

/// Emit a FormBuilder call: `form.<method>(positional, opts)`.
/// Method-name remapping: the Rails `textarea` alias normalizes to
/// `text_area` (spinel-blog's runtime exposes the underscore form
/// only). `submit` with no positional arg gets a leading `nil` —
/// matches spinel-blog's `form.submit(nil, class: "...")` shape.
/// Trailing opts hash, if present, runs through
/// `simplify_class_array` so `class: ["base", {…}]` collapses to
/// `class: "base"` (the conditional clauses drop today; an
/// errors-aware composition lands when a fixture forces it).
pub(super) fn emit_form_builder_call(
    recv_name: Symbol,
    kind: FormBuilderMethod,
    args: &[Expr],
) -> Expr {
    let method_name = match kind {
        FormBuilderMethod::Label => "label",
        FormBuilderMethod::TextField => "text_field",
        FormBuilderMethod::TextArea => "text_area",
        FormBuilderMethod::Submit => "submit",
    };
    let mut new_args: Vec<Expr> = args.iter().map(simplify_arg_class_array).collect();
    if matches!(kind, FormBuilderMethod::Submit) {
        // `form.submit class: "..."` had no positional in the source;
        // spinel runtime expects `form.submit(label, opts)`. Insert
        // a leading nil when the first arg isn't a positional value.
        let first_is_hash = matches!(
            new_args.first().map(|a| &*a.node),
            Some(ExprNode::Hash { .. }),
        );
        if new_args.is_empty() || first_is_hash {
            new_args.insert(0, nil_lit());
        }
    }
    send(Some(var_ref(recv_name)), method_name, new_args, None, true)
}

/// Walk one positional/opts arg and simplify a `class:` Hash entry
/// whose value is a Rails-style `["base", {cond_class: pred, …}]`
/// array. Replaces the array with just the base string. Other entries
/// pass through unchanged.
fn simplify_arg_class_array(arg: &Expr) -> Expr {
    let ExprNode::Hash { entries, braced } = &*arg.node else {
        return arg.clone();
    };
    let new_entries: Vec<(Expr, Expr)> = entries
        .iter()
        .map(|(k, v)| {
            let is_class_key = matches!(
                &*k.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "class",
            );
            if is_class_key {
                (k.clone(), simplify_class_array(v))
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect();
    Expr::new(
        arg.span,
        ExprNode::Hash { entries: new_entries, braced: *braced },
    )
}

/// `["base_string", {cond_class: pred, …}]` → `"base_string"`.
/// Anything else passes through unchanged.
fn simplify_class_array(v: &Expr) -> Expr {
    let ExprNode::Array { elements, .. } = &*v.node else {
        return v.clone();
    };
    let Some(first) = elements.first() else {
        return v.clone();
    };
    if matches!(&*first.node, ExprNode::Lit { value: Literal::Str { .. } }) {
        return first.clone();
    }
    v.clone()
}
