//! `block_param` signature refinement — when a method declares `&block`
//! and forwards it to another method whose block signature is known,
//! refine the forwarder's `MethodDef.signature` block slot to match.
//!
//! Stage 3 of issue #25 (Proc-as-IR-value). Without this pass, the
//! forwarder's rust2 emit falls back to the `Box<dyn FnOnce()>`
//! placeholder from `render_block_param_placeholder` — too loose to
//! satisfy a callee that expects `FnOnce(String) -> i32`. After this
//! pass, the forwarder's signature carries the callee's block sig,
//! and `render_block_closure_param` produces the matching shape.
//!
//! Scope: same-class only. Cross-class dispatch resolution requires
//! the full class registry (not just `&[LibraryClass]`) and is the
//! stage-3b follow-on. Single forwarding site assumption — when a
//! body forwards to multiple callees with conflicting block sigs,
//! the first wins (callers can disambiguate via explicit RBS).

use crate::dialect::LibraryClass;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::{Param, ParamKind, Ty};

/// Refine every method's block-param signature across `classes`.
/// Same-class lookup only — a method that forwards `&blk` to a sibling
/// in the same `LibraryClass` whose block sig is known will pick up
/// that sig. Methods without `block_param`, or whose body doesn't
/// forward to a known callee, are unchanged.
pub fn propagate(classes: &mut [LibraryClass]) {
    for class in classes.iter_mut() {
        propagate_one(class);
    }
}

/// Single-class pass — exported so callers driving the per-class
/// emit pipeline can invoke it directly.
pub fn propagate_one(class: &mut LibraryClass) {
    // Snapshot callee block sigs by method name. Reading happens
    // before mutation so the loop below can both read sibling sigs
    // and mutate the current method without borrow conflicts.
    let callee_block_sigs: Vec<(String, Ty)> = class
        .methods
        .iter()
        .filter_map(|m| {
            let sig = m.signature.as_ref()?;
            let block_ty = callee_block_ty(sig)?;
            Some((m.name.as_str().to_string(), block_ty))
        })
        .collect();

    for m in class.methods.iter_mut() {
        let Some(bp) = m.block_param.as_ref() else {
            continue;
        };
        // Already has a typed block sig — RBS or earlier refinement
        // won. Don't overwrite.
        if m.signature
            .as_ref()
            .and_then(callee_block_ty)
            .is_some()
        {
            continue;
        }
        let forwarded_to = first_forwarded_callee(&m.body, &bp.name);
        let Some(callee_name) = forwarded_to else { continue };
        let Some(block_ty) = callee_block_sigs
            .iter()
            .find(|(n, _)| n == callee_name.as_str())
            .map(|(_, ty)| ty.clone())
        else {
            continue;
        };
        refine_signature_block(m, block_ty);
    }
}

/// Extract the block-Ty from a method's signature, if present. RBS
/// carries the block sig as a `ParamKind::Block` entry in the params
/// vec (see `src/rbs.rs`); reading that path keeps the refined sigs
/// shaped the same as RBS-derived sigs so rust2 emit's existing
/// `render_block_closure_param` consumes both uniformly.
fn callee_block_ty(sig: &Ty) -> Option<Ty> {
    let Ty::Fn { params, .. } = sig else { return None };
    params
        .iter()
        .find(|p| matches!(p.kind, ParamKind::Block))
        .map(|p| p.ty.clone())
}

/// Walk `body` for the first `Send { block: Some(Var(name)), .. }`
/// where the Var's name matches the forwarder's block-param name.
/// Returns the callee method name (the Send's `method` field). The
/// forwarding idiom always targets a specific callee — multi-forward
/// methods are rare and the first-hit strategy is sufficient.
fn first_forwarded_callee(body: &Expr, block_name: &Symbol) -> Option<Symbol> {
    match &*body.node {
        ExprNode::Send { method, block: Some(block_expr), .. } => {
            if let ExprNode::Var { name, .. } = &*block_expr.node {
                if name == block_name {
                    return Some(method.clone());
                }
            }
            None
        }
        ExprNode::Seq { exprs } => exprs
            .iter()
            .find_map(|e| first_forwarded_callee(e, block_name)),
        ExprNode::If { cond, then_branch, else_branch } => {
            first_forwarded_callee(cond, block_name)
                .or_else(|| first_forwarded_callee(then_branch, block_name))
                .or_else(|| first_forwarded_callee(else_branch, block_name))
        }
        ExprNode::Assign { value, .. } => first_forwarded_callee(value, block_name),
        ExprNode::Return { value } => first_forwarded_callee(value, block_name),
        _ => None,
    }
}

/// Stamp the inferred block sig onto the forwarder's signature. If
/// the forwarder had no signature, fabricate a minimal one with
/// Untyped params/ret — only the block slot carries real info. If
/// it had a non-Fn signature (unusual), leave it alone.
fn refine_signature_block(m: &mut crate::dialect::MethodDef, block_ty: Ty) {
    let block_param = Param {
        name: Symbol::new("block"),
        ty: block_ty.clone(),
        kind: ParamKind::Block,
    };
    match m.signature.take() {
        Some(Ty::Fn { mut params, block: _, ret, effects }) => {
            params.retain(|p| !matches!(p.kind, ParamKind::Block));
            params.push(block_param);
            m.signature = Some(Ty::Fn {
                params,
                block: Some(Box::new(block_ty)),
                ret,
                effects,
            });
        }
        None => {
            let placeholder_params: Vec<Param> = m
                .params
                .iter()
                .map(|p| Param {
                    name: p.name.clone(),
                    ty: Ty::Untyped,
                    kind: ParamKind::Required,
                })
                .chain(std::iter::once(block_param))
                .collect();
            m.signature = Some(Ty::Fn {
                params: placeholder_params,
                block: Some(Box::new(block_ty)),
                ret: Box::new(Ty::Untyped),
                effects: crate::effect::EffectSet::default(),
            });
        }
        Some(other) => {
            // Non-Fn signature (unexpected for a MethodDef) — put it
            // back and skip. The forwarder keeps the placeholder emit.
            m.signature = Some(other);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param as DialectParam};
    use crate::effect::EffectSet;
    use crate::expr::{Expr, ExprNode};
    use crate::ident::{ClassId, Symbol};
    use crate::span::Span;
    use crate::ty::{Param, ParamKind, Ty};

    fn typed_callee_block_sig() -> Ty {
        // Block sig: `(String, untyped) -> Nil` — Session#each-shaped.
        Ty::Fn {
            params: vec![
                Param {
                    name: Symbol::new("k"),
                    ty: Ty::Str,
                    kind: ParamKind::Required,
                },
                Param {
                    name: Symbol::new("v"),
                    ty: Ty::Untyped,
                    kind: ParamKind::Required,
                },
            ],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: EffectSet::pure(),
        }
    }

    fn callee_each() -> MethodDef {
        // Method `each` with the typed block sig — mirrors Session#each.
        let block_sig = typed_callee_block_sig();
        MethodDef {
            name: Symbol::from("each"),
            receiver: MethodReceiver::Instance,
            params: vec![],
            body: Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] }),
            signature: Some(Ty::Fn {
                params: vec![Param {
                    name: Symbol::new("block"),
                    ty: block_sig.clone(),
                    kind: ParamKind::Block,
                }],
                block: Some(Box::new(block_sig)),
                ret: Box::new(Ty::Nil),
                effects: EffectSet::pure(),
            }),
            effects: EffectSet::pure(),
            enclosing_class: Some(Symbol::from("Holder")),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        }
    }

    fn forwarder_calls_each() -> MethodDef {
        // `def forwarder(&blk); each(&blk); end` shape.
        let blk = Symbol::from("blk");
        let send_each = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("each"),
                args: vec![],
                block: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: crate::ident::VarId(0), name: blk.clone() },
                )),
                parenthesized: true,
            },
        );
        MethodDef {
            name: Symbol::from("forwarder"),
            receiver: MethodReceiver::Instance,
            params: vec![],
            body: send_each,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: Some(Symbol::from("Holder")),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: Some(DialectParam::positional(blk)),
        }
    }

    fn holder_class(methods: Vec<MethodDef>) -> LibraryClass {
        LibraryClass {
            name: ClassId(Symbol::from("Holder")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods,
            origin: None,
            constants: Vec::new(),
        }
    }

    #[test]
    fn forwarder_picks_up_callee_block_sig() {
        let mut class = holder_class(vec![callee_each(), forwarder_calls_each()]);
        propagate_one(&mut class);
        let fwd = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "forwarder")
            .unwrap();
        let block_ty = callee_block_ty(fwd.signature.as_ref().expect("fabricated sig"))
            .expect("block-Param now present in sig");
        match &block_ty {
            Ty::Fn { params, ret, .. } => {
                assert_eq!(params.len(), 2, "callee block has 2 args");
                assert!(matches!(params[0].ty, Ty::Str));
                assert!(matches!(**ret, Ty::Nil));
            }
            other => panic!("expected Ty::Fn block sig, got {other:?}"),
        }
    }

    #[test]
    fn forwarder_without_known_callee_unchanged() {
        let mut fwd = forwarder_calls_each();
        // Edit Send target to a name not in the class — nothing to look up.
        if let ExprNode::Send { method, .. } = &mut *fwd.body.node {
            *method = Symbol::from("absent");
        }
        let mut class = holder_class(vec![callee_each(), fwd]);
        propagate_one(&mut class);
        let fwd = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "forwarder")
            .unwrap();
        assert!(
            fwd.signature.is_none(),
            "unresolved callee → no signature fabrication, got {:?}",
            fwd.signature
        );
    }

    #[test]
    fn existing_typed_block_sig_not_overwritten() {
        let mut fwd = forwarder_calls_each();
        // Pre-populate with a distinct block sig (single Int arg).
        let pre_sig = Ty::Fn {
            params: vec![Param {
                name: Symbol::new("block"),
                ty: Ty::Fn {
                    params: vec![Param {
                        name: Symbol::new("x"),
                        ty: Ty::Int,
                        kind: ParamKind::Required,
                    }],
                    block: None,
                    ret: Box::new(Ty::Untyped),
                    effects: EffectSet::pure(),
                },
                kind: ParamKind::Block,
            }],
            block: None,
            ret: Box::new(Ty::Untyped),
            effects: EffectSet::pure(),
        };
        fwd.signature = Some(pre_sig.clone());
        let mut class = holder_class(vec![callee_each(), fwd]);
        propagate_one(&mut class);
        let fwd = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "forwarder")
            .unwrap();
        assert_eq!(fwd.signature.as_ref(), Some(&pre_sig));
    }
}
