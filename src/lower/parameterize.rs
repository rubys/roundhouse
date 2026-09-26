//! ActiveSupport `String#parameterize` grounding: a zero-arg
//! `parameterize` on a receiver the analyzer stamped `Str` →
//! `Inflector.parameterize(recv)` (lobsters' Story#title_as_url:
//! `self.title.parameterize`). The AS original is a String core_ext
//! reopen only the CRuby overlay can host — spinel AOT has no method
//! to dispatch, so the site was an unresolved call there. `Inflector`
//! is the same home the pluralize grounding uses. Scope mirrors
//! `transaction_ground`: model bodies only (the corpus' one site);
//! separator-kwarg forms and untyped receivers stay verbatim — on
//! CRuby the overlay serves them, on strict targets they are honest
//! residue.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_parameterize_grounding(app: &mut App) {
    for model in &mut app.models {
        for item in &mut model.body {
            match item {
                crate::dialect::ModelBodyItem::Method { method, .. } => rewrite(&mut method.body),
                // A block-form callback spliced from a concern — lobsters'
                // Token: `after_initialize do self.token ||=
                // TypeID.new(self.class.to_s.parameterize) … end`. It is
                // lowered to a hook method later, so it is still this
                // item kind here, and skipping it left the call
                // unresolved on spinel.
                crate::dialect::ModelBodyItem::Unknown { expr, .. } => rewrite(expr),
                _ => {}
            }
        }
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    if let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *expr.node {
        if method.as_str() == "parameterize"
            && args.is_empty()
            && block.is_none()
            && recv
                .as_ref()
                .is_some_and(|r| matches!(r.ty.as_ref(), Some(Ty::Str)))
        {
            let receiver = recv.take().unwrap();
            *recv = Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("Inflector")] },
            ));
            args.push(receiver);
            *parenthesized = true;
        }
    }
}
