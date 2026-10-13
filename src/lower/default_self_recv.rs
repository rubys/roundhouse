//! Qualify bare sends in parameter defaults as `self.<method>`.
//!
//! A default like `badge: user.memberships.unread.count` is evaluated
//! with the method's receiver as `self`, so Ruby resolves bare `user`
//! to `self.user`. Spinel AOT, faced with the same bare send in a
//! default thunk, can pick a *different* `user` in the program
//! (`ApplicationController#user`) and then cast the receiver wrong —
//! `sp_User * = ApplicationController_user((ApplicationController *)
//! (Subscription *))`, which the C build refuses.
//!
//! Emitting an explicit `self.user` keeps the resolution on the
//! enclosing class. Only bare sends inside defaults are rewritten;
//! method bodies, already-qualified calls, constants, and locals (Var
//! nodes) are left alone. The inserted `SelfRef` is stamped with the
//! enclosing class type so receiver-type dispatch in typed emitters
//! still sees a class (post-analyze inserts would otherwise leave
//! `ty: None`).

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{Association, MethodDef, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ty::Ty;

pub fn apply_default_self_recv(app: &mut App) -> Vec<Diagnostic> {
    for model in &mut app.models {
        let self_ty = Ty::Class {
            id: model.name.clone(),
            args: vec![].into(),
        };
        for item in &mut model.body {
            match item {
                ModelBodyItem::Method { method, .. } => {
                    rewrite_method_defaults(method, &self_ty);
                }
                ModelBodyItem::Scope { scope, .. } => {
                    for p in &mut scope.params {
                        if let Some(default) = &mut p.default {
                            rewrite(default, &self_ty);
                        }
                    }
                }
                ModelBodyItem::Association {
                    assoc: Association::HasMany { extension, .. },
                    ..
                } => {
                    for m in extension.iter_mut() {
                        rewrite_method_defaults(m, &self_ty);
                    }
                }
                _ => {}
            }
        }
    }
    for lc in app
        .library_classes
        .iter_mut()
        .chain(app.rails_application.iter_mut())
    {
        let self_ty = Ty::Class {
            id: lc.name.clone(),
            args: vec![].into(),
        };
        for method in &mut lc.methods {
            rewrite_method_defaults(method, &self_ty);
        }
    }
    Vec::new()
}

/// Skip class-side methods: the inserted `SelfRef` is stamped as an
/// instance `Ty::Class`, and typed emitters (Kotlin) then take the
/// instance dispatch path. Until IR can represent class-object `self`,
/// leave class-method defaults unchanged.
fn rewrite_method_defaults(method: &mut MethodDef, self_ty: &Ty) {
    if method.receiver == MethodReceiver::Class {
        return;
    }
    for p in &mut method.params {
        if let Some(default) = &mut p.default {
            rewrite(default, self_ty);
        }
    }
}

fn rewrite(expr: &mut Expr, self_ty: &Ty) {
    expr.node
        .for_each_child_mut(&mut |c| rewrite(c, self_ty));
    let ExprNode::Send { recv, .. } = &mut *expr.node else {
        return;
    };
    if recv.is_some() {
        return;
    }
    let mut s = Expr::new(expr.span, ExprNode::SelfRef);
    s.ty = Some(self_ty.clone());
    *recv = Some(s);
}
