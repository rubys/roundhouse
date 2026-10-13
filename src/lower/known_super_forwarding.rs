//! `def m(...)` whose only use of the forward is `super`, restated with
//! the destination's own parameters when that destination is a stdlib
//! method whose signature is known.
//!
//! campfire's `WebPush::Connections::Stages` (included into
//! `class HTTP < Net::HTTP`) overrides two of CRuby net/http's private
//! transport hooks and forwards to them:
//!
//! ```ruby
//! def begin_transport(...)
//!   @stage = :checking
//!   super.tap { @stage = :sent }
//! end
//! ```
//!
//! `...` has a native carrier on the Ruby family only. Here the callee is
//! fixed: `super` from the override reaches `Net::HTTP#begin_transport`,
//! and CRuby declares that `begin_transport(req)`, with no keywords and no
//! block. A forward that can only carry what the destination takes is
//! that destination's parameter list, so the override becomes
//! `def begin_transport(req)` and its `super` becomes `super(req)`, which
//! means the same thing on every target, CRuby included.
//!
//! The rewrite applies only when all of these hold:
//!
//! - the formal list is exactly `...`;
//! - every forward in the body is `super` or `super(...)`;
//! - the method is defined on a class whose direct superclass is in the
//!   table below, or on a module whose sole includer is such a class;
//! - the table names the method.
//!
//! Anything else keeps its `...` and the target's refusal.

use crate::app::App;
use crate::dialect::{MethodDef, Param};
use crate::expr::{Expr, ExprNode};
use crate::ident::{ClassId, Symbol, VarId};

/// Stdlib methods reached by `super` from app overrides, with their
/// positional parameters, as CRuby declares them. A name here is a
/// claim about every lane: the Ruby family has CRuby's net/http, and the
/// spinel lane's `runtime/spinel/net_http.rb` defines both hooks.
const KNOWN: &[(&str, &str, &[&str])] = &[
    ("Net::HTTP", "begin_transport", &["req"]),
    ("Net::HTTP", "connect", &[]),
];

pub fn restate(app: &mut App) {
    let sole = app.sole_includer_of_modules();
    let parent_of = |id: &ClassId| -> Option<String> {
        app.library_classes
            .iter()
            .find(|lc| &lc.name == id)
            .and_then(|lc| lc.parent.as_ref())
            .map(|p| p.0.as_str().trim_start_matches("::").to_string())
    };
    let mut destination: Vec<Option<String>> = Vec::new();
    for lc in &app.library_classes {
        let host = if lc.is_module {
            sole.get(&lc.name).cloned()
        } else {
            Some(lc.name.clone())
        };
        destination.push(host.as_ref().and_then(parent_of));
    }
    for (lc, parent) in app.library_classes.iter_mut().zip(destination) {
        let Some(parent) = parent else { continue };
        for m in &mut lc.methods {
            let Some((_, _, names)) = KNOWN
                .iter()
                .find(|(class, method, _)| *class == parent && *method == m.name.as_str())
            else {
                continue;
            };
            restate_method(m, names);
        }
    }
}

fn restate_method(m: &mut MethodDef, names: &[&str]) {
    if !(m.params.len() == 1 && m.params[0].forwarding) || m.has_anonymous_block {
        return;
    }
    if !forwards_only_to_super(&m.body) {
        return;
    }
    m.params = names
        .iter()
        .map(|n| Param {
            name: Symbol::from(*n),
            default: None,
            keyword: false,
            rest: false,
            forwarding: false,
            from_keyword: false,
            from_kwrest: false,
        })
        .collect();
    rewrite_supers(&mut m.body, names);
}

/// Every `ForwardArgs` in the body is the sole argument of a `super`.
fn forwards_only_to_super(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::ForwardArgs => false,
        ExprNode::Super { args: Some(args) }
            if args.len() == 1 && matches!(&*args[0].node, ExprNode::ForwardArgs) =>
        {
            true
        }
        _ => {
            let mut ok = true;
            e.node.for_each_child(&mut |c| ok = ok && forwards_only_to_super(c));
            ok
        }
    }
}

fn rewrite_supers(e: &mut Expr, names: &[&str]) {
    let forwarded = match &*e.node {
        ExprNode::Super { args: None } => true,
        ExprNode::Super { args: Some(args) } => {
            args.len() == 1 && matches!(&*args[0].node, ExprNode::ForwardArgs)
        }
        _ => false,
    };
    if forwarded {
        let span = e.span;
        let args = names
            .iter()
            .map(|n| Expr::new(span, ExprNode::Var { id: VarId(0), name: Symbol::from(*n) }))
            .collect();
        *e.node = ExprNode::Super { args: Some(args) };
        return;
    }
    e.node.for_each_child_mut(&mut |c| rewrite_supers(c, names));
}
