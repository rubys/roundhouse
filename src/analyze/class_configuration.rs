//! Class-object state is a separate typing domain from controller instance
//! fields and schema columns. Only the two validated finite method roles
//! enter here; no arbitrary method parameters/defaults or annotations.

use std::collections::HashMap;

use crate::dialect::{ClassConfigurationRole, Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::{ClassId, Symbol};
use crate::ty::{Param, Ty};

use super::{Analyzer, Ctx, union_of};

impl Analyzer {
    pub(super) fn analyze_class_configuration(&self, controllers: &mut [Controller]) {
        let mut types: HashMap<(ClassId, Symbol), Ty> = HashMap::new();
        for controller in controllers.iter_mut() {
            for item in &mut controller.body {
                let ControllerBodyItem::ClassIvarInit { expr, carrier, .. } = item else {
                    continue;
                };
                if let ExprNode::Assign { value, .. } = &mut *expr.node {
                    seed_empty_hashes(value, &empty_hash());
                }
                self.body_typer().analyze_expr(expr, &Ctx::default());
                if let ExprNode::Assign {
                    target: LValue::Ivar { name },
                    value,
                } = &*expr.node
                {
                    if let Some(ty) = &value.ty {
                        types
                            .entry((carrier.clone(), name.clone()))
                            .and_modify(|old| {
                                *old = union_of(old.clone(), ty.clone());
                            })
                            .or_insert_with(|| ty.clone());
                    }
                }
            }
        }

        // Runtime call sites are real type evidence, not a reason to force
        // the boot literals' narrower signature. The ordinary concern fold
        // already joins includer/subclass calls under the carrier's key.
        for controller in controllers.iter() {
            for item in &controller.body {
                if let ControllerBodyItem::ClassMethod {
                    method,
                    configuration_slot: key,
                    configuration_role: ClassConfigurationRole::Writer,
                    ..
                } = item
                {
                    if let Some(ty @ Ty::Hash { .. }) = self
                        .inferred_params
                        .get(&(key.0.clone(), method.name.clone()))
                        .and_then(|params| params.first())
                    {
                        types
                            .entry(key.clone())
                            .and_modify(|old| {
                                *old = union_of(old.clone(), ty.clone());
                            })
                            .or_insert_with(|| ty.clone());
                    }
                }
            }
        }

        for controller in controllers {
            for item in &mut controller.body {
                let ControllerBodyItem::ClassMethod {
                    method,
                    configuration_slot,
                    configuration_role,
                    ..
                } = item
                else {
                    continue;
                };
                let ty = types
                    .get(configuration_slot)
                    .cloned()
                    .unwrap_or_else(empty_hash);
                let mut ctx = Ctx {
                    self_ty: Some(Ty::Class {
                        id: controller.name.clone(),
                        args: vec![],
                    }),
                    ..Ctx::default()
                };
                // Methods inherit, initialized values do not: every class
                // object may still have an unset slot, even with a parent.
                ctx.ivar_bindings
                    .insert(configuration_slot.1.clone(), union_of(ty.clone(), Ty::Nil));
                if *configuration_role == ClassConfigurationRole::Writer {
                    for param in &method.params {
                        ctx.local_bindings.insert(param.name.clone(), ty.clone());
                    }
                }
                seed_empty_hashes(&mut method.body, &ty);
                self.body_typer().analyze_expr(&mut method.body, &ctx);
                method.effects = self.collect_effects(&mut method.body, &ctx);
                method.signature = Some(Ty::Fn {
                    params: method
                        .params
                        .iter()
                        .map(|p| Param {
                            name: p.name.clone(),
                            ty: ty.clone(),
                            kind: p.ty_kind(),
                        })
                        .collect(),
                    block: None,
                    ret: Box::new(method.body.ty.clone().unwrap_or(Ty::Untyped)),
                    effects: method.effects.clone(),
                });
            }
        }
    }
}

fn empty_hash() -> Ty {
    Ty::Hash {
        key: Box::new(Ty::Bottom),
        value: Box::new(Ty::Bottom),
    }
}

fn seed_empty_hashes(expr: &mut Expr, ty: &Ty) {
    if matches!(&*expr.node, ExprNode::Hash { entries, .. } if entries.is_empty()) {
        expr.ty = Some(ty.clone());
    }
    expr.node
        .for_each_child_mut(&mut |child| seed_empty_hashes(child, ty));
}
