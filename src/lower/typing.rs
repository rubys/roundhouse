//! Shared typing helpers for lowerers — typed literal constructors,
//! signature builders, and the body-typer wrapper. Used by
//! `model_to_library`, `controller_to_library`, and `view_to_library`
//! to record types the lowerer knows by construction (literals, schema
//! columns, action returns) without leaving them for a separate pass
//! to rediscover.

use crate::dialect::MethodDef;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::{Param as TyParam, ParamKind, Ty};

/// Attach a known type to an Expr. Lowerers use this when the type is
/// statically known by construction.
pub fn with_ty(mut e: Expr, ty: Ty) -> Expr {
    e.ty = Some(ty);
    e
}

pub fn lit_str(s: String) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: s } }),
        Ty::Str,
    )
}

pub fn lit_sym(name: Symbol) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: name } }),
        Ty::Sym,
    )
}

pub fn lit_int(value: i64) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Int { value } }),
        Ty::Int,
    )
}

pub fn lit_float(value: f64) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value } }),
        Ty::Float,
    )
}

pub fn lit_bool(value: bool) -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value } }),
        Ty::Bool,
    )
}

pub fn nil_lit() -> Expr {
    with_ty(
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        Ty::Nil,
    )
}

/// Build a `Ty::Fn` signature from positional (name, type) pairs and a return type.
/// Effects default to pure — callers refine if needed.
pub fn fn_sig(params: Vec<(Symbol, Ty)>, ret: Ty) -> Ty {
    fn_sig_with_block(params, None, ret)
}

/// Variant that records the block-param type a method yields. The
/// body-typer's `block_params_for` consults this when the receiver is
/// a registered class — lets framework stubs (form_with → FormBuilder,
/// ErrorCollection.each → Str) declare their block shape inline
/// rather than hardcoding each one in the typer.
pub fn fn_sig_with_block(
    params: Vec<(Symbol, Ty)>,
    block: Option<Ty>,
    ret: Ty,
) -> Ty {
    Ty::Fn {
        params: params
            .into_iter()
            .map(|(name, ty)| TyParam {
                name,
                ty: ty.into(),
                kind: ParamKind::Required,
            })
            .collect(),
        block: block.map(std::sync::Arc::new),
        ret: std::sync::Arc::new(ret),
        effects: crate::effect::EffectSet::pure(),
    }
}

/// Run the body-typer over a method's body, seeded by the method's
/// signature (params), enclosing class (`self_ty`), an ivar map, and
/// a class registry. Used by every lowerer that wants its output
/// fully typed.
pub fn type_method_body(
    method: &mut MethodDef,
    classes: &std::collections::HashMap<ClassId, crate::analyze::ClassInfo>,
    ivar_bindings: &std::collections::HashMap<Symbol, Ty>,
) {
    let typer = crate::analyze::BodyTyper::new(classes);
    let mut ctx = crate::analyze::Ctx::default();
    if let Some(Ty::Fn { params, .. }) = &method.signature {
        for (param, sig) in method.params.iter().zip(params.iter()) {
            ctx.local_bindings.insert(param.name.clone(), (*sig.ty).clone());
        }
    }
    if let Some(enclosing) = &method.enclosing_class {
        let id = ClassId(enclosing.clone());
        ctx.self_ty = Some(Ty::Class {
            id: id.clone(),
            args: vec![].into(),
        });
        // Seed class constants so Value Const resolution can see
        // locally-owned bare names (test-helper splice). Without this,
        // `qualify_resolved_path` expands `WEB_PUSH_PUBLIC_TEST_IP` back
        // to `DnsTestHelper::WEB_PUSH_PUBLIC_TEST_IP`.
        if let Some(info) = classes.get(&id) {
            if !info.constants.is_empty() {
                let global = crate::analyze::ConstScope::default();
                ctx.constants = global.with_own(info.constants.clone());
            }
        }
    }
    if matches!(method.receiver, crate::dialect::MethodReceiver::Instance) {
        for (name, ty) in ivar_bindings {
            ctx.ivar_bindings.insert(name.clone(), ty.clone());
        }
    }
    ctx.annotate_self_dispatch = true;
    typer.analyze_expr(&mut method.body, &ctx);
}
