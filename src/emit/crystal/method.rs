//! Crystal `def` emission with type annotations.
//!
//! Drives off `MethodDef.signature: Option<Ty::Fn>`. When the signature
//! is fully typed (no `Untyped` reachable through params/return), emit
//! the annotated form `def name(a : T, b : U) : R`. When any position
//! is `Untyped`, drop annotations entirely and let Crystal's inference
//! fill in — partial annotation triggers Crystal's stricter checking
//! and would surface false-positive errors at the gap.

use std::fmt::Write;

use super::expr::emit_expr;
use super::shared::escape_ident;
use super::ty::{crystal_ty, has_untyped};
use crate::dialect::{MethodDef, MethodReceiver};
use crate::ty::Ty;

/// Emit a single `MethodDef` as Crystal source (trailing newline
/// included). Mirrors `super::super::ruby::emit_method` in surface
/// shape; adds Crystal-specific signature annotations.
pub fn emit_method(m: &MethodDef) -> String {
    let prefix = match m.receiver {
        MethodReceiver::Instance => "",
        MethodReceiver::Class => "self.",
    };

    // Decide whether to emit type annotations. The signature is the
    // authority; when missing or carrying `Untyped`, fall back to
    // bare `def name(args)`.
    let annotate = m
        .signature
        .as_ref()
        .map(|sig| !sig_has_untyped(sig))
        .unwrap_or(false);

    let params = render_params(m, annotate);
    let ret_clause = if annotate {
        if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
            // Drop the return-type annotation when the declared
            // return references the enclosing class — these are
            // self-typed methods (RBS `instance` / `self` types,
            // which the Roundhouse RBS parser doesn't have first-
            // class. AR Base's `def self.all : Array[Base]` is the
            // canonical case. Article inherits `all`; Crystal's
            // strict-typing infers `Array(Article)` per-subclass-
            // call, but a literal `Array(Base)` annotation rejects
            // that (Array is invariant). Letting Crystal infer
            // per-subclass produces the right per-class type.
            if returns_enclosing_class(ret, m.enclosing_class.as_ref()) {
                String::new()
            } else {
                // Drop the explicit return-type annotation for a few
                // collection-shaped methods whose bodies delegate to
                // Crystal stdlib (Hash#size, Array#size) returning
                // `Int32` even though the RBS contract is the broader
                // `Integer` (which would map to `Int64`). Letting
                // Crystal infer keeps the body and signature in sync
                // without forcing per-target body rewrites.
                let mname = m.name.as_str();
                if matches!(mname, "length" | "size")
                    && matches!(&**ret, crate::ty::Ty::Int)
                {
                    String::new()
                } else {
                    format!(" : {}", crystal_ty(ret))
                }
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let mut out = String::new();
    writeln!(out, "def {prefix}{}{params}{ret_clause}", m.name).unwrap();
    // Return-ty back-prop for empty `{}` / `[]` tails: when the
    // declared return is `Hash(K, V)` / `Array(E)` and the body-typer
    // left the literal as `Hash<Var, Var>`, stamp the declared shape
    // before emit so `emit_expr` renders `{} of K => V` rather than
    // the String=>String default. Same contract as Go's
    // `backprop_return_ty_to_tail` and the runtime_src pre-type stamp
    // — without it, `default_url_options : Hash(Symbol, String)` with
    // body `{}` fails Crystal typecheck (FV compare-extra crystal).
    let body = if let Some(Ty::Fn { ret, .. }) = m.signature.as_ref() {
        let mut body = m.body.clone();
        crate::analyze::stamp_expected_on_empty_return_tail(&mut body, ret);
        body
    } else {
        m.body.clone()
    };
    // Synth `[]` index reader body: per-arm `@field` reads must NOT
    // get the auto-`.not_nil!` Ivar bridge — fresh-from-`.new` records
    // have unset ivars, and the bridge would crash on lookup. Crystal
    // emit toggles a thread-local flag for the duration of this
    // method's body. Other methods (initialize, action handlers,
    // etc.) keep the bridge — its narrowing is sound there.
    let body_text = if m.name.as_str() == "[]"
        && matches!(m.receiver, MethodReceiver::Instance)
    {
        super::expr::with_suppressed_ivar_not_nil(|| emit_expr(&body))
    } else {
        emit_expr(&body)
    };
    // Crystal disallows `@ivar` references inside `def self.X` (class
    // methods on a metaclass). Ruby's `module_function` shares ivars
    // across class methods; the Crystal analog is `@@class_var`.
    // Rewrite `@x` → `@@x` for class-method bodies. The pattern only
    // matches when `@` is followed by an identifier char (skipping
    // lone `@` or `@@x` already rewritten).
    let body_text = if matches!(m.receiver, MethodReceiver::Class) {
        rewrite_ivars_to_class_vars(&body_text)
    } else {
        body_text
    };
    for line in body_text.lines() {
        if line.is_empty() {
            out.push('\n');
        } else {
            writeln!(out, "  {line}").unwrap();
        }
    }
    out.push_str("end\n");
    out
}

fn rewrite_ivars_to_class_vars(body: &str) -> String {
    // Not pushed byte by byte into a String: a byte above 0x7f is half a UTF-8 character, not a char
    let mut out: Vec<u8> = Vec::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    // Track whether we're inside a string literal (`"..."`). The
    // promoter walks raw emitted Crystal text — JSON-bearing strings
    // (`"@hotwired/turbo-rails"` in the importmap) must not get the
    // `@`→`@@` rewrite, or the JSON ships malformed at runtime. Skip
    // characters inside strings; track escapes so an embedded `\"`
    // doesn't close the string prematurely.
    let mut in_string = false;
    let mut string_escape = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            out.push(c);
            i += 1;
            if string_escape {
                string_escape = false;
            } else if c == b'\\' {
                string_escape = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        if c == b'"' {
            in_string = true;
            out.push(b'"');
            i += 1;
            continue;
        }
        if c == b'@' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            // Already `@@` — emit as-is, advance past both.
            if next == b'@' {
                out.push(b'@');
                out.push(b'@');
                i += 2;
                continue;
            }
            // `@<ident>` — promote to `@@<ident>`.
            if next.is_ascii_alphabetic() || next == b'_' {
                out.extend_from_slice(b"@@");
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8(out).expect("the rewrite only adds ASCII to UTF-8 text")
}

fn render_params(m: &MethodDef, annotate: bool) -> String {
    if m.params.is_empty() {
        return String::new();
    }
    let sig_params = if annotate {
        if let Some(Ty::Fn { params, .. }) = m.signature.as_ref() {
            Some(params)
        } else {
            None
        }
    } else {
        None
    };

    let ps: Vec<String> = m
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = escape_ident(p.name.as_str());
            let default_clause = match &p.default {
                Some(default) => format!(" = {}", emit_expr(default)),
                None => String::new(),
            };
            match sig_params.as_ref().and_then(|sp| sp.get(i)) {
                Some(sig_p) => format!("{name} : {}{default_clause}", crystal_ty(&sig_p.ty)),
                None => format!("{name}{default_clause}"),
            }
        })
        .collect();
    format!("({})", ps.join(", "))
}

fn sig_has_untyped(sig: &Ty) -> bool {
    let Ty::Fn { params, ret, .. } = sig else {
        return true;
    };
    params.iter().any(|p| has_untyped(&p.ty)) || has_untyped(ret)
}

/// True when the return type references the enclosing class — direct
/// (`Ty::Class { id == enclosing }`), wrapped in Array (`Array<Self>`),
/// wrapped in a `Self | Nil` Union (`SelfOrNil`), or nested under
/// these. Mirrors RBS's `instance` / `self` self-type concept; used
/// to opt out of the Crystal return-type annotation so per-subclass
/// inference can narrow the result to the actual subclass type.
fn returns_enclosing_class(ret: &Ty, enclosing: Option<&crate::ident::Symbol>) -> bool {
    let Some(enclosing) = enclosing else {
        return false;
    };
    match ret {
        Ty::Class { id, .. } => id.0.as_str() == enclosing.as_str(),
        Ty::Array { elem } => returns_enclosing_class(elem, Some(enclosing)),
        Ty::Union { variants } => variants
            .iter()
            .any(|v| returns_enclosing_class(v, Some(enclosing))),
        _ => false,
    }
}

#[cfg(test)]
mod empty_hash_return_tests {
    use super::*;
    use crate::dialect::MethodDef;
    use crate::expr::{Expr, ExprNode};
    use crate::ident::Symbol;
    use crate::span::Span;
    use std::sync::Arc;

    fn empty_hash_method(ret: Ty) -> MethodDef {
        let body = Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: vec![],
                kwargs: false,
            },
        );
        MethodDef {
            name: Symbol::from("default_url_options"),
            receiver: MethodReceiver::Instance,
            visibility: Default::default(),
            params: vec![],
            unsupported_formals: None,
            has_anonymous_block: false,
            block_param: None,
            name_span: Span::synthetic(),
            body,
            signature: Some(Ty::Fn {
                params: vec![].into(),
                block: None,
                ret: Arc::new(ret),
                effects: Default::default(),
            }),
            effects: Default::default(),
            enclosing_class: Some(Symbol::from("ActionController::Base")),
            kind: Default::default(),
            is_async: false,
            mutates_self: false,
        }
    }

    #[test]
    fn empty_hash_return_emits_of_declared_key_and_value() {
        let ret = Ty::Hash {
            key: Arc::new(Ty::Sym),
            value: Arc::new(Ty::Str),
        };
        let out = emit_method(&empty_hash_method(ret));
        assert!(
            out.contains("def default_url_options : Hash(Symbol, String)"),
            "missing annotated signature:\n{out}"
        );
        assert!(
            out.contains("{} of Symbol => String"),
            "empty hash must honor declared return, not String=>String default:\n{out}"
        );
        assert!(
            !out.contains("{} of String => String"),
            "must not emit the untyped empty-hash default:\n{out}"
        );
    }

    /// Trailing `if cond; {}; else; {}; end` must stamp both arms so
    /// Crystal does not fall back to `{} of String => String`.
    #[test]
    fn empty_hash_if_branch_return_emits_of_declared_key_and_value() {
        let ret = Ty::Hash {
            key: Arc::new(Ty::Sym),
            value: Arc::new(Ty::Str),
        };
        let empty = || {
            Expr::new(
                Span::synthetic(),
                ExprNode::Hash {
                    entries: vec![],
                    kwargs: false,
                },
            )
        };
        let mut m = empty_hash_method(ret);
        m.body = Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit {
                        value: crate::expr::Literal::Bool { value: true },
                    },
                ),
                then_branch: empty(),
                else_branch: empty(),
            },
        );
        let out = emit_method(&m);
        assert!(
            out.contains("{} of Symbol => String"),
            "If-arm empty hash must honor declared return:\n{out}"
        );
        assert!(
            !out.contains("{} of String => String"),
            "must not emit String-keyed default from If arms:\n{out}"
        );
    }
}
