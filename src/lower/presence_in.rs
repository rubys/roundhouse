//! ActiveSupport `Object#presence_in` grounding: `value.presence_in(list)`
//! → `ActiveSupport.presence_in(value, list)`.
//!
//! Rails defines it on Object as `in?(another) ? self : nil` — the
//! allow-list spelling for a value that came off the wire. campfire's
//! `Accounts::UsersController` writes the whole role check with it:
//!
//! ```ruby
//! { role: params.require(:user)[:role].presence_in(%w[ member administrator ]) || "member" }
//! ```
//!
//! Like `parameterize`, the original is a core_ext reopen only the
//! CRuby overlay could host, so every other target had an unresolved
//! call. `ActiveSupport` is the home its `presence` sibling already
//! uses, and one shared `runtime/ruby` body prices all thirteen targets.
//!
//! Receiver-shape only, no type gate: `presence_in` is declared on
//! Object, so there is no receiver type that would make the site mean
//! something else — unlike `parameterize`, which shares its name with
//! nothing but is still stamped `Str` to stay inside what the
//! `Inflector` body can answer.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

pub fn apply_presence_in_grounding(app: &mut App) {
    super::for_each_hook_body(app, &mut rewrite);
    for view in &mut app.views {
        rewrite(&mut view.body);
    }
}

fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);
    let span = expr.span;
    if let ExprNode::Send { recv, method, args, block, parenthesized } = &mut *expr.node {
        if method.as_str() == "presence_in"
            && args.len() == 1
            && block.is_none()
            && recv.is_some()
        {
            let receiver = recv.take().unwrap();
            *recv = Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] },
            ));
            args.insert(0, receiver);
            *parenthesized = true;
        }
    }
}
