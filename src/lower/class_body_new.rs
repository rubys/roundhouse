//! A bare `new` in a CLASS BODY is the class's own constructor.
//!
//! `self` in a class body — and in a `def self.` method — is the class,
//! so Ruby reads `new(name: "bell")` there as `Sound.new(name: "bell")`.
//! campfire's `Sound::BUILTIN` is fifty-six of them in one array
//! literal, and the emit replayed the bare spelling:
//!
//! ```text
//! app/models/sound.rb:39: unsupported call:
//!   node 95214 (CallNode `new`) recv=-/ty-1 argc=1 arg0ty28
//! ```
//!
//! A receiverless call has no receiver to resolve against, and `new`
//! is not a free function anywhere. The same array with the receiver
//! spelled out compiles and runs on spinel today (probed), which is
//! what makes this ours rather than a compiler gap.
//!
//! Scoped to the two positions where implicit self IS the class:
//! class-body constant initializers (they run at load time, in the
//! class body) and class-side method bodies. An INSTANCE method's bare
//! `new` is a NoMethodError in Ruby too, so there is nothing there to
//! preserve.
//!
//! Only `new`. `create` / `create!` are the same shape on a MODEL class
//! body, but they are also association-scope constructors that
//! `lower::scope_chain` already claims at implicit self
//! (`SELF_CONSTRUCTORS`), and no corpus app writes one in a constant.
//! When one does, it belongs beside this — not in a second walk.

use crate::app::App;
use crate::dialect::{MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_class_body_new_lowering(app: &mut App) {
    for model in &mut app.models {
        let owner = model.name.0.as_str().to_string();
        for item in &mut model.body {
            match item {
                // A model's constants arrive as replayed class-body
                // code rather than on a field of their own.
                ModelBodyItem::Unknown { expr, .. } => {
                    if matches!(&*expr.node, ExprNode::Assign { target: crate::expr::LValue::Const { .. }, .. }) {
                        rewrite(expr, &owner);
                    }
                }
                ModelBodyItem::Method { method, .. }
                    if matches!(method.receiver, MethodReceiver::Class) =>
                {
                    rewrite(&mut method.body, &owner);
                }
                _ => {}
            }
        }
    }
    for lc in &mut app.library_classes {
        let owner = lc.name.0.as_str().to_string();
        for (_, value) in &mut lc.constants {
            rewrite(value, &owner);
        }
        for m in &mut lc.methods {
            if matches!(m.receiver, MethodReceiver::Class) {
                rewrite(&mut m.body, &owner);
            }
        }
    }
}

fn rewrite(expr: &mut Expr, owner: &str) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, owner));
    let ExprNode::Send { recv: recv @ None, method, .. } = &mut *expr.node else { return };
    if method.as_str() != "new" {
        return;
    }
    let span = expr.span;
    let path: Vec<Symbol> = owner.split("::").map(Symbol::from).collect();
    let mut konst = Expr::new(span, ExprNode::Const { path });
    // The receiver is the class ITSELF, which is what the expression's
    // own type already says the result is — no new type is invented
    // here, and the send keeps whatever analyze stamped on it.
    konst.ty = expr.ty.clone().map(|t| match t {
        crate::ty::Ty::Class { id, args } => crate::ty::Ty::Class { id, args },
        _ => crate::ty::Ty::Untyped,
    });
    if matches!(konst.ty, Some(crate::ty::Ty::Untyped)) {
        konst.ty = None;
    }
    *recv = Some(konst);
}
