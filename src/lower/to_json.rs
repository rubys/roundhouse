//! `<hash-or-array>.to_json` → `JSON.generate(<hash-or-array>)`.
//!
//! `Hash#to_json` is a core_ext reopen — Ruby's json library adds it,
//! ActiveSupport replaces it — and a reopened builtin is the one shape
//! no strict target can host and spinel cannot dispatch on. The value
//! it produces has a home already: the bundled JSON package every
//! emitted tree requires through `runtime/json_impl.rb`, whose
//! `generate` takes the same collection and answers the same String.
//! Same move as `parameterize` → `Inflector.parameterize`, and it
//! spends no new runtime surface.
//!
//! campfire's `Webhook#payload` builds a nested Hash and ends
//! `}.to_json`, which is the whole request body a bot receives — an
//! unresolved call there is not a late NoMethodError on some route, it
//! is every webhook delivery.
//!
//! GATE: a receiver the analyzer typed `Ty::Hash` or `Ty::Array`. Those
//! are exactly what `JSON.generate` serializes directly. A MODEL
//! receiver is deliberately excluded — `record.to_json` is Rails'
//! `as_json`-then-encode, which the `as_json_*` passes own and which
//! answers a different string (the model's declared shape, not its
//! ivars) — and so is an untyped one, where the receiver could be
//! anything and the rewrite would be a guess.
//!
//! DIVERGENCE: ActiveSupport's `to_json` walks `as_json` first, so a
//! Time value renders in Rails' ISO-8601 form where `JSON.generate`
//! would refuse it. The corpus receiver holds strings, integers and
//! nested hashes of the same, and a value JSON cannot encode raises
//! rather than rendering wrongly. Recorded in docs/pipeline/runtime.md.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;
use crate::ty::Ty;

pub fn apply_to_json_lowering(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *expr.node else {
        return;
    };
    if method.as_str() != "to_json" || !args.is_empty() || block.is_some() {
        return;
    }
    if !recv
        .as_ref()
        .is_some_and(|r| matches!(r.ty.as_ref(), Some(Ty::Hash { .. } | Ty::Array { .. })))
    {
        return;
    }
    let receiver = recv.take().expect("checked above");
    *recv = Some(Expr::new(span, ExprNode::Const { path: vec![Symbol::from("JSON")] }));
    *method = Symbol::from("generate");
    args.push(receiver);
    *parenthesized = true;
    expr.ty = Some(Ty::Str);
}
