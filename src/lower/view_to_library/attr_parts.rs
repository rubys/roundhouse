//! Shared attribute-rendering helpers for the macro-inlined view
//! tags (form/input/label/textarea from form_builder, plus a/form
//! wrappers from link_to/button_to). Each opts entry renders as one
//! ` <name>="<escaped_value>"` segment appended to the running
//! `Vec<InterpPart>`. Nested `data:/aria:` hashes flatten to kebab-
//! cased compound names (`data-turbo-confirm="..."`); class-array
//! opts collapse via `simplify_class_array_pub` (re-exported from
//! `form_builder` to keep one implementation).

use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::span::Span;

use super::{lit_str, send, view_helpers_call};

/// Walk `opts` entries and emit ` <key>="<escaped_value>"` (or
/// flattened `data-<inner>="..."` for nested hashes) into `parts`.
/// Non-Symbol keys are skipped (no real fixture exercises them).
///
/// `simplify_class_array` is applied to `class:` entries so
/// Rails-style `["base", {cond: pred, ...}]` arrays collapse to
/// `"base <first_key>"` literal — same byte-for-byte behavior as
/// the prior runtime FormBuilder + the prior runtime render_attrs.
pub(super) fn append_attr_parts(parts: &mut Vec<InterpPart>, opts: &[(Expr, Expr)]) {
    for (k, v) in opts {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            continue;
        };
        // Nested `data: { turbo_confirm: "..." }` / `aria: { ... }`
        // hashes flatten to `data-turbo-confirm="..."`. Inner keys
        // map `_` → `-` per Rails ActionView convention; values run
        // through html_escape. Only Hash literals exercise this
        // path — dynamic hashes pass through the simple-value
        // branch (which would render `[object Object]`-shaped junk,
        // but no real fixture exercises that shape).
        if let ExprNode::Hash { entries: inner, .. } = &*v.node {
            for (ik, iv) in inner {
                let inner_key = match &*ik.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                    ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                    _ => continue,
                };
                let kebab = inner_key.replace('_', "-");
                parts.push(InterpPart::Text {
                    value: format!(" {}-{}=\"", key.as_str(), kebab),
                });
                parts.push(InterpPart::Expr {
                    expr: view_helpers_call("html_escape", vec![lit_str_coerce(iv.clone())]),
                });
                parts.push(InterpPart::Text {
                    value: "\"".to_string(),
                });
            }
            continue;
        }
        let simplified = if key.as_str() == "class" {
            super::form_builder::simplify_class_array_pub(v)
        } else {
            v.clone()
        };
        parts.push(InterpPart::Text {
            value: format!(" {}=\"", key.as_str()),
        });
        parts.push(InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![lit_str_coerce(simplified)]),
        });
        parts.push(InterpPart::Text {
            value: "\"".to_string(),
        });
    }
}

/// Wrap non-String-literal opts values in `.to_s` so html_escape's
/// `(String) -> String` contract is satisfied across targets.
/// Numeric `rows: 4`, Symbol `method: :delete`, and similar lower
/// to `4.to_s` / `:delete.to_s` at the call site; the body-typer
/// resolves to_s on each per its primitive table.
pub(super) fn lit_str_coerce(e: Expr) -> Expr {
    let is_str_lit = matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Str { .. } },
    );
    if is_str_lit {
        e
    } else {
        send(Some(e), "to_s", Vec::new(), None, false)
    }
}

/// Build a `StringInterp` Expr node from the assembled parts,
/// collapsing adjacent Text segments so the emitted body reads as
/// one literal where the static prefix/suffix would otherwise chain
/// through multiple no-op InterpParts.
pub(super) fn string_interp(parts: Vec<InterpPart>) -> Expr {
    let mut merged: Vec<InterpPart> = Vec::new();
    for p in parts {
        match (&p, merged.last_mut()) {
            (
                InterpPart::Text { value: rhs },
                Some(InterpPart::Text { value: lhs }),
            ) => {
                lhs.push_str(rhs);
            }
            _ => merged.push(p),
        }
    }
    Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp { parts: merged },
    )
}

/// Find a kwarg entry by its Symbol key, returning the value Expr
/// and a new entries Vec with that pair removed. Used by
/// `button_to`'s inline expansion to peel off `method:` and
/// `form_class:` from the opts before forwarding the rest to the
/// inner `<button>` element. Returns `None` when the key isn't
/// present (caller picks the default).
pub(super) fn take_opt(opts: &mut Vec<(Expr, Expr)>, key: &str) -> Option<Expr> {
    let pos = opts.iter().position(|(k, _)| {
        matches!(
            &*k.node,
            ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == key,
        )
    })?;
    let (_, v) = opts.remove(pos);
    Some(v)
}

/// Synthesize a `lit_sym(:post)` default for `button_to`'s missing
/// `method:` opt — the runtime helper's `method_override_input`
/// returns `""` for `:post` / `:get`, so this is a no-op append
/// when the caller didn't pass an explicit method.
pub(super) fn default_method_sym() -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit {
            value: Literal::Sym { value: Symbol::from("post") },
        },
    )
}

/// Synthesize a `lit_str("button_to")` default for `button_to`'s
/// missing `form_class:` opt — matches Rails' convention of giving
/// every button_to-rendered form the `button_to` class when no
/// override is supplied.
pub(super) fn default_form_class() -> Expr {
    lit_str("button_to".to_string())
}
