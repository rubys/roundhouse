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
                format!(" : {}", crystal_ty(ret))
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    let mut out = String::new();
    writeln!(out, "def {prefix}{}{params}{ret_clause}", m.name).unwrap();
    // Synth `[]` index reader body: per-arm `@field` reads must NOT
    // get the auto-`.not_nil!` Ivar bridge — fresh-from-`.new` records
    // have unset ivars, and the bridge would crash on lookup. Crystal
    // emit toggles a thread-local flag for the duration of this
    // method's body. Other methods (initialize, action handlers,
    // etc.) keep the bridge — its narrowing is sound there.
    let body_text = if m.name.as_str() == "[]"
        && matches!(m.receiver, MethodReceiver::Instance)
    {
        super::expr::with_suppressed_ivar_not_nil(|| emit_expr(&m.body))
    } else {
        emit_expr(&m.body)
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
    let mut out = String::with_capacity(body.len());
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
            out.push(c as char);
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
            out.push('"');
            i += 1;
            continue;
        }
        if c == b'@' && i + 1 < bytes.len() {
            let next = bytes[i + 1];
            // Already `@@` — emit as-is, advance past both.
            if next == b'@' {
                out.push('@');
                out.push('@');
                i += 2;
                continue;
            }
            // `@<ident>` — promote to `@@<ident>`.
            if next.is_ascii_alphabetic() || next == b'_' {
                out.push_str("@@");
                i += 1;
                continue;
            }
        }
        out.push(c as char);
        i += 1;
    }
    out
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
