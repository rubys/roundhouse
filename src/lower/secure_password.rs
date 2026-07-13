//! `has_secure_password` — synthesize the methods Rails' macro
//! provides, in the shared model lowering (all targets): the
//! authenticator (`authenticate`, or `authenticate_<attr>` for a
//! custom attribute) returning the record or `false`, plus the
//! plaintext virtual-attribute accessors (`password` / `password=` /
//! `password_confirmation` / `password_confirmation=`).
//!
//! The bodies call the bcrypt gem's own surface (`BCrypt::Password.
//! create/new`) VERBATIM — deliberately not a roundhouse intrinsic:
//! per the spin mirror-naming policy (spinel#1753), a future
//! `spinel-bcrypt` native-`[[build]]` package claims `require
//! "bcrypt"` and satisfies this exact contract, so the emitted code
//! runs unchanged on the CRuby/JRuby trees (which load the real gem
//! via the overlay's guarded require) and on spinel once the package
//! lands. Until then strict targets and plain spinel carry the calls
//! as ONE named runtime seam — the bucket-3 posture from the gem-fate
//! taxonomy, and the "has_secure_password expected" entry on the AOT
//! probe's peel list.
//!
//! The analyzer already types this surface
//! (`register_has_secure_password`); signatures here mirror it —
//! writers take the plaintext `Str`, and the authenticator returns
//! the model instance (the dominant truthy use; the runtime `false`
//! arm is the analyzer's deliberate simplification, kept in
//! agreement here).
//!
//! Shared home of what used to be the ruby-family emit pass
//! `apply_secure_password_lowering`. Strict no-op for marker-free
//! apps (the blog).

use super::model_to_library::{fn_sig, push_synth_instance_method};
use crate::dialect::{AccessorKind, MethodDef, Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// Synthesize the `has_secure_password` method family onto `model`'s
/// lowered methods. Custom methods in the model body win, as do names
/// an earlier synthesizer claimed.
pub(crate) fn push_secure_password_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let Some(attr) = secure_password_attr(&model.body) else {
        return;
    };
    let digest = Symbol::from(format!("{}_digest", attr.as_str()));
    let confirmation = Symbol::from(format!("{}_confirmation", attr.as_str()));
    // Rails names the authenticator after the attribute, except the
    // default `password` which gets the bare `authenticate`.
    let auth_name = if attr.as_str() == "password" {
        Symbol::from("authenticate")
    } else {
        Symbol::from(format!("authenticate_{}", attr.as_str()))
    };
    let plain = Symbol::from("unencrypted_password");
    let self_ty = Ty::Class { id: model.name.clone(), args: vec![] };
    let plaintext_ty = Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    push_synth_instance_method(
        methods,
        model,
        auth_name,
        vec![Param::positional(plain.clone())],
        authenticate_body(&digest),
        Some(fn_sig(vec![(plain.clone(), Ty::Str)], self_ty)),
        AccessorKind::Method,
        false,
    );
    // Plaintext virtual attribute: reader is a plain ivar read (nil
    // until a writer runs in this process — the digest column is the
    // persistent side), writer stores the plaintext AND the bcrypt
    // digest.
    push_synth_instance_method(
        methods,
        model,
        attr.clone(),
        Vec::new(),
        ivar_read(&attr),
        Some(fn_sig(vec![], plaintext_ty.clone())),
        AccessorKind::AttributeReader,
        false,
    );
    push_synth_instance_method(
        methods,
        model,
        Symbol::from(format!("{}=", attr.as_str())),
        vec![Param::positional(plain.clone())],
        plaintext_writer_body(&attr, &digest),
        Some(fn_sig(vec![(plain, Ty::Str)], Ty::Nil)),
        AccessorKind::Method,
        true,
    );
    push_synth_instance_method(
        methods,
        model,
        confirmation.clone(),
        Vec::new(),
        ivar_read(&confirmation),
        Some(fn_sig(vec![], plaintext_ty)),
        AccessorKind::AttributeReader,
        false,
    );
    let value = Symbol::from("value");
    push_synth_instance_method(
        methods,
        model,
        Symbol::from(format!("{}=", confirmation.as_str())),
        vec![Param::positional(value.clone())],
        plain_ivar_assign(&confirmation, &value),
        Some(fn_sig(vec![(value, Ty::Str)], Ty::Nil)),
        AccessorKind::AttributeWriter,
        true,
    );
}

/// The secure-password attribute name when the model body declares
/// `has_secure_password` (first positional symbol, default
/// `password`), else None. Mirrors analyze's registration scan.
/// `pub(crate)` for the permit-writer filter (model_to_library), which
/// counts the synthesized plaintext writers as assignable.
pub(crate) fn secure_password_attr(body: &[ModelBodyItem]) -> Option<Symbol> {
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "has_secure_password" {
            continue;
        }
        let attr = args
            .iter()
            .find_map(|a| match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            })
            .unwrap_or_else(|| Symbol::from("password"));
        return Some(attr);
    }
    None
}

fn sp_expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn ivar_read(name: &Symbol) -> Expr {
    sp_expr(ExprNode::Ivar { name: name.clone() })
}

fn plain_ivar_assign(name: &Symbol, param: &Symbol) -> Expr {
    sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: name.clone() },
        value: sp_expr(ExprNode::Var { id: VarId(0), name: param.clone() }),
    })
}

/// `BCrypt::Password.new(@<digest>) == unencrypted_password ? self : false`.
fn authenticate_body(digest: &Symbol) -> Expr {
    let wrapped = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const {
            path: vec![Symbol::from("BCrypt"), Symbol::from("Password")],
        })),
        method: Symbol::from("new"),
        args: vec![ivar_read(digest)],
        block: None,
        parenthesized: true,
    });
    let cmp = sp_expr(ExprNode::Send {
        recv: Some(wrapped),
        method: Symbol::from("=="),
        args: vec![sp_expr(ExprNode::Var {
            id: VarId(0),
            name: Symbol::from("unencrypted_password"),
        })],
        block: None,
        parenthesized: false,
    });
    sp_expr(ExprNode::If {
        cond: cmp,
        then_branch: sp_expr(ExprNode::SelfRef),
        else_branch: sp_expr(ExprNode::Lit { value: Literal::Bool { value: false } }),
    })
}

/// The plaintext writer Rails' macro provides:
///   `@<attr> = v; @<attr>_digest = BCrypt::Password.create(v).to_s unless v.nil?`
/// (`.to_s` because BCrypt::Password subclasses String but the digest
/// column stores plain text). Nil skips digest generation, mirroring
/// Rails' blank-guard closely enough for the login/rehash paths.
fn plaintext_writer_body(attr: &Symbol, digest: &Symbol) -> Expr {
    let value_var = || {
        sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from("unencrypted_password") })
    };
    let store_plain = sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: attr.clone() },
        value: value_var(),
    });
    let create = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const {
            path: vec![Symbol::from("BCrypt"), Symbol::from("Password")],
        })),
        method: Symbol::from("create"),
        args: vec![value_var()],
        block: None,
        parenthesized: true,
    });
    let digest_str = sp_expr(ExprNode::Send {
        recv: Some(create),
        method: Symbol::from("to_s"),
        args: Vec::new(),
        block: None,
        parenthesized: false,
    });
    let guarded_digest = sp_expr(ExprNode::If {
        cond: sp_expr(ExprNode::Send {
            recv: Some(value_var()),
            method: Symbol::from("nil?"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        }),
        then_branch: sp_expr(ExprNode::Lit { value: Literal::Nil }),
        else_branch: sp_expr(ExprNode::Assign {
            target: LValue::Ivar { name: digest.clone() },
            value: digest_str,
        }),
    });
    sp_expr(ExprNode::Seq { exprs: vec![store_plain, guarded_digest] })
}
