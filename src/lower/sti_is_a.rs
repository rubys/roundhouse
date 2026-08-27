//! `room.is_a?(Rooms::Open)` — an STI type test — is a read of the
//! inheritance COLUMN.
//!
//! Rails answers it from the object's class, and an STI object's class
//! is whatever its `type` column said when the row was loaded. So the
//! honest lowering is the column comparison the row itself carries:
//!
//! ```text
//! is_a?(Rooms::Open)        ->  type == "Rooms::Open"
//! room.is_a?(Rooms::Closed) ->  room.type == "Rooms::Closed"
//! ```
//!
//! campfire's `Room#open?` / `#closed?` / `#direct?` are three of these
//! and they steer real behaviour — which rooms a new user is granted,
//! which sidebar section a room lands in. They arrived at the spinel
//! build as an unsupported call, because a RECEIVERLESS `is_a?` has no
//! receiver to resolve against:
//!
//! ```text
//! app/models/room.rb:498: unsupported call:
//!   node 87536 (CallNode `is_a?`) recv=-/ty-1 argc=1 arg0ty48
//! ```
//!
//! **Spelling the receiver would have been the wrong fix, and probing
//! is what showed it.** `self.is_a?(Sub)` compiles on spinel — and
//! answers FALSE for a `Sub` receiver when the method it sits in is
//! defined on `Base`, while the same test from outside answers true
//! (12-line repro, filed upstream). A build that stops is a wall; a
//! predicate that quietly says "this open room is not open" is the
//! failure that looks like success. The column comparison needs no
//! `is_a?` at all, and it is the same answer on every target — the
//! ruby-family lanes hydrate the subclass, the strict ones do not, and
//! `type` is what both of them have.
//!
//! DESCENDANTS are folded in: a test against a subclass that itself has
//! subclasses is true for any of them, so the rewrite becomes an
//! `include?` over the whole set rather than one equality. (No corpus
//! app nests STI two deep today; the set-of-one path is what campfire
//! takes.)
//!
//! Only classes `sti_scope::sti_bases` calls STI — a library class whose
//! parent chain reaches a Model whose table carries the inheritance
//! column. `x.is_a?(String)`, `image.is_a?(Symbol)` and every other
//! ordinary type test are left exactly alone.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

/// The column Rails writes an STI row's class name into.
const INHERITANCE_COLUMN: &str = "type";

pub fn apply_sti_is_a_lowering(app: &mut App) {
    let bases = crate::lower::sti_scope::sti_bases(app);
    if bases.is_empty() {
        return;
    }
    // Subclass → every name a row of that class can carry in `type`:
    // itself plus anything descending from it.
    let mut names: HashMap<ClassId, Vec<String>> = HashMap::new();
    for sub in bases.keys() {
        let mut set: Vec<String> = vec![sub.0.as_str().to_string()];
        for other in bases.keys() {
            if other != sub && descends_from(app, other, sub) {
                set.push(other.0.as_str().to_string());
            }
        }
        set.sort();
        names.insert(sub.clone(), set);
    }
    super::for_each_hook_body(app, &mut |e| rewrite(e, &names));
    for view in &mut app.views {
        rewrite(&mut view.body, &names);
    }
}

/// Does `class_id`'s parent chain reach `ancestor`?
fn descends_from(app: &App, class_id: &ClassId, ancestor: &ClassId) -> bool {
    let mut cursor = app
        .library_classes
        .iter()
        .find(|lc| &lc.name == class_id)
        .and_then(|lc| lc.parent.clone());
    let mut hops = 0;
    while let Some(parent) = cursor {
        if hops > 8 {
            return false;
        }
        if &parent == ancestor {
            return true;
        }
        cursor = app
            .library_classes
            .iter()
            .find(|lc| lc.name == parent)
            .and_then(|lc| lc.parent.clone());
        hops += 1;
    }
    false
}

fn rewrite(expr: &mut Expr, names: &HashMap<ClassId, Vec<String>>) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, names));
    let ExprNode::Send { recv, method, args, .. } = &*expr.node else { return };
    if method.as_str() != "is_a?" && method.as_str() != "kind_of?" {
        return;
    }
    if args.len() != 1 {
        return;
    }
    let ExprNode::Const { path } = &*args[0].node else { return };
    let named = ClassId(Symbol::from(
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"),
    ));
    let Some(type_names) = names.get(&named) else { return };
    let span = expr.span;
    // `type` on the receiver — bare when the test was receiverless,
    // which is the implicit self it already stood for.
    let mut type_read = Expr::new(
        span,
        ExprNode::Send {
            recv: recv.clone(),
            method: Symbol::from(INHERITANCE_COLUMN),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    type_read.ty = Some(Ty::Str);
    let lit = |s: &str| {
        let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } });
        e.ty = Some(Ty::Str);
        e
    };
    let node = if type_names.len() == 1 {
        ExprNode::Send {
            recv: Some(type_read),
            method: Symbol::from("=="),
            args: vec![lit(&type_names[0])],
            block: None,
            parenthesized: false,
        }
    } else {
        let mut set = Expr::new(
            span,
            ExprNode::Array {
                elements: type_names.iter().map(|n| lit(n)).collect(),
                style: crate::expr::ArrayStyle::Brackets,
            },
        );
        set.ty = Some(Ty::Array { elem: Box::new(Ty::Str) });
        ExprNode::Send {
            recv: Some(set),
            method: Symbol::from("include?"),
            args: vec![type_read],
            block: None,
            parenthesized: true,
        }
    };
    *expr.node = node;
    expr.ty = Some(Ty::Bool);
}

/// Names the pass consults, for the gate to assert against without
/// rebuilding the walk.
#[allow(dead_code)]
pub(crate) fn sti_type_names(app: &App) -> HashSet<String> {
    crate::lower::sti_scope::sti_bases(app)
        .keys()
        .map(|k| k.0.as_str().to_string())
        .collect()
}
