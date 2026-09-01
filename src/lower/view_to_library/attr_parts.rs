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
pub(crate) fn append_attr_parts(parts: &mut Vec<InterpPart>, opts: &[(Expr, Expr)]) {
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
        //
        // ONLY those two prefixes. MEASURED: `tag.form(foo: { a: 1 })`
        // renders `foo="a 1"` — Rails expands a nested hash for `data`
        // and `aria` and for nothing else. Flattening every hash turned
        // lobsters' `form_with html: { id: "edit_story" }` into
        // `html-id="edit_story"`, an attribute no browser reads. (The
        // `foo="a 1"` spelling itself stays unmodeled — the generic
        // branch below renders Ruby's `to_s` — because no corpus app
        // writes a non-data/aria hash attribute.)
        let flattens = matches!(key.as_str(), "data" | "aria");
        if let (true, ExprNode::Hash { entries: inner, .. }) = (flattens, &*v.node) {
            for (ik, iv) in inner {
                let inner_key = match &*ik.node {
                    ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
                    ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
                    _ => continue,
                };
                // A nil sub-value is NO ATTRIBUTE, the same rule the
                // outer level follows — MEASURED: `tag.form(data: {
                // nothing: nil })` renders `<form>`, not
                // `data-nothing=""`. (`false` is NOT nil here: Rails
                // renders `data-turbo="false"`, because the boolean-
                // attribute rule does not reach `data-*` sub-keys.)
                if matches!(&*iv.node, ExprNode::Lit { value: Literal::Nil }) {
                    continue;
                }
                let kebab = inner_key.replace('_', "-");
                push_escaped_attr(parts, &format!("{}-{}", key.as_str(), kebab), iv);
            }
            continue;
        }
        // `data:`/`aria:` whose value is NOT a literal hash — campfire's
        // `data: composer_data_options(room)`. Whether it expands to
        // `data-*` pairs depends on whether the value IS a hash, which
        // only the value knows, so defer the same dispatch Rails makes
        // to the runtime `render_attrs` (already the haml path's
        // renderer, and already compiling on every strict target).
        // Rendering it the ordinary way instead ships the hash's `to_s`
        // as one `data="{…}"` attribute — this branch's own comment
        // above used to say no fixture reached the shape, and campfire's
        // composer now does.
        if flattens {
            parts.push(InterpPart::Expr {
                expr: view_helpers_call(
                    "render_attrs",
                    vec![Expr::new(
                        v.span,
                        ExprNode::Hash {
                            entries: vec![(k.clone(), v.clone())],
                            kwargs: false,
                        },
                    )],
                ),
            });
            continue;
        }
        if let Some(decided) = tag_option_parts(key.as_str(), v) {
            parts.extend(decided);
            continue;
        }
        let simplified = if key.as_str() == "class" {
            super::form_builder::simplify_class_array_pub(v)
        } else {
            v.clone()
        };
        push_escaped_attr(parts, key.as_str(), &simplified);
    }
}

/// The generic ` key="value"` tail both attribute loops share. A
/// literal (or interpolated-string) value renders inline — it cannot be
/// nil at run time. A DYNAMIC value gets Rails' `elsif !value.nil?`
/// rule as an inline conditional, because nil means NO ATTRIBUTE:
/// campfire's layout writes `tag.meta name: "vapid-public-key",
/// content: <config read>`, and with no key configured Rails renders
/// `<meta name="vapid-public-key">` where the unguarded emit said
/// `content=""` — the room-page comparator's find. Same guard shape as
/// the nested-`data:` conditional form_builder's loop already carries
/// (an `If` inside an `InterpPart::Expr`, proven on every lane).
pub(crate) fn push_escaped_attr(parts: &mut Vec<InterpPart>, key: &str, value: &Expr) {
    let open = InterpPart::Text { value: format!(" {key}=\"") };
    let escaped = InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![lit_str_coerce(value.clone())]),
    };
    let close = InterpPart::Text { value: "\"".to_string() };
    if matches!(
        &*value.node,
        ExprNode::Lit { .. } | ExprNode::StringInterp { .. }
    ) {
        parts.push(open);
        parts.push(escaped);
        parts.push(close);
        return;
    }
    parts.push(InterpPart::Expr {
        expr: Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: send(Some(value.clone()), "nil?", Vec::new(), None, false),
                then_branch: lit_str(String::new()),
                else_branch: string_interp(vec![open, escaped, close]),
            },
        ),
    });
}

/// The part of Rails' `tag_options` that decides whether an attribute
/// renders at all, and how. `Some(parts)` means the rule OWNS this
/// attribute's rendering (possibly to nothing); `None` means render it
/// the ordinary `key="value"` way.
///
/// Two rules, both about attributes that are not name/value pairs:
///
///   * a BOOLEAN attribute renders as `key="key"` when truthy and is
///     OMITTED when falsy — never `disabled="false"`, which a browser
///     reads as disabled. lobsters' comment box passes
///     `disabled: !@user` on its textarea, submit and preview button,
///     so a logged-in reply page shipped three dead controls.
///   * a literal `nil` value is Rails' "no such attribute" (`next
///     unless value`). Rendered as `key=""` it can mean the opposite:
///     `open=""` opens a `<details>`.
///
/// Lives here, called from both attribute loops (this file's and
/// form_builder's) — the rule is Rails' and belongs in one place even
/// while the two loops keep their own `data:`-hash handling.
pub(crate) fn tag_option_parts(key: &str, v: &Expr) -> Option<Vec<InterpPart>> {
    if is_boolean_attr(key) {
        return Some(match &*v.node {
            ExprNode::Lit { value: Literal::Bool { value: true } } => {
                vec![InterpPart::Text { value: format!(" {key}=\"{key}\"") }]
            }
            ExprNode::Lit { value: Literal::Bool { value: false } }
            | ExprNode::Lit { value: Literal::Nil } => Vec::new(),
            _ => vec![InterpPart::Expr {
                expr: Expr::new(
                    Span::synthetic(),
                    ExprNode::If {
                        cond: v.clone(),
                        then_branch: lit_str(format!(" {key}=\"{key}\"")),
                        else_branch: lit_str(String::new()),
                    },
                ),
            }],
        });
    }
    if matches!(&*v.node, ExprNode::Lit { value: Literal::Nil }) {
        return Some(Vec::new());
    }
    None
}

/// Rails' `BOOLEAN_ATTRIBUTES` (ActionView's TagHelper): attributes
/// whose mere presence is the value. Kept verbatim so the rendering
/// rule matches Rails for any of them, not just the ones a fixture
/// happens to use.
fn is_boolean_attr(key: &str) -> bool {
    matches!(
        key,
        "allowfullscreen"
            | "allowpaymentrequest"
            | "async"
            | "autofocus"
            | "autoplay"
            | "checked"
            | "compact"
            | "controls"
            | "declare"
            | "default"
            | "defaultchecked"
            | "defaultmuted"
            | "defaultselected"
            | "defer"
            | "disabled"
            | "enabled"
            | "formnovalidate"
            | "hidden"
            | "indeterminate"
            | "inert"
            | "ismap"
            | "itemscope"
            | "loop"
            | "multiple"
            | "muted"
            | "nohref"
            | "nomodule"
            | "noresize"
            | "noshade"
            | "novalidate"
            | "nowrap"
            | "open"
            | "pauseonexit"
            | "playsinline"
            | "pubdate"
            | "readonly"
            | "required"
            | "reversed"
            | "scoped"
            | "seamless"
            | "selected"
            | "sortable"
            | "truespeed"
            | "typemustmatch"
            | "visible"
    )
}

/// Wrap non-String-literal opts values in `.to_s` so html_escape's
/// `(String) -> String` contract is satisfied across targets.
/// Numeric `rows: 4`, Symbol `method: :delete`, and similar lower
/// to `4.to_s` / `:delete.to_s` at the call site; the body-typer
/// resolves to_s on each per its primitive table.
pub(crate) fn lit_str_coerce(e: Expr) -> Expr {
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
pub(crate) fn string_interp(parts: Vec<InterpPart>) -> Expr {
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
