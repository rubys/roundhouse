//! `recv.try(:m)` guards DEFINEDNESS, not nilness.
//!
//! ActiveSupport:
//!
//! ```ruby
//! def try(method_name = nil, *args, &b)
//!   if method_name.nil? ... elsif respond_to?(method_name)
//!     public_send(method_name, *args, &b)
//!   end
//! end
//! ```
//!
//! (plus `NilClass#try`, which answers nil for everything). Ingest used
//! to ground it to `recv && recv.m` — the `&.` desugar. The two agree
//! exactly when the receiver either is nil or DOES define the method,
//! which is most sites and not all:
//!
//! ```ruby
//! # campfire's own turbo_test_helper, over [room, :messages]
//! streambles.collect { |s| s.try(:to_gid_param) || s }.join(":")
//! ```
//!
//! `:messages` is not nil and answers no `to_gid_param`, so Rails
//! returns nil and takes the `|| s` arm. The nil guard called it and
//! raised — `undefined method 'to_gid_param' for an instance of Symbol`
//! — which is campfire's `test_creating_a_message_broadcasts_the_
//! message_to_the_room`, the app's own assertion of the behaviour
//! `scripts/campfire-cable-walk` demonstrates.
//!
//! `respond_to?` IS THE ONE ANSWER THIS PIPELINE CANNOT EMIT — a
//! runtime dispatch question, and the strict targets compile the test
//! tree too. What it CAN do is ask the tree which classes define the
//! name, because that set is closed:
//!
//! * every class descending from a common ancestor `B` defines it, and
//!   nothing outside does -> `recv.is_a?(B) ? recv.m : nil`
//! * a handful of unrelated classes define it -> the same, over a
//!   disjunction
//! * nothing in the tree defines it -> the call can only ever answer
//!   nil, so fold to `nil`
//! * too many unrelated definers to name -> keep the nil guard, and say
//!   so in the ledger rather than emit a twenty-arm `is_a?` chain
//!
//! WHY THE NARROWING IS SAFE WHERE THE NIL GUARD WAS NOT: `nil.is_a?(B)`
//! is false, so a nil receiver still answers nil — the narrowing does
//! everything the nil guard did, and answers nil for the non-nil
//! receiver that does not define the method instead of raising on it.
//!
//! The bound that keeps this honest is that `m` is a LITERAL at every
//! corpus site (`:username`, `"is_moderator"`). A computed method name
//! is left as a plain `try` send for the analyzer to report.

use std::collections::{HashMap, HashSet};

use crate::app::App;
use crate::dialect::MethodReceiver;
use crate::expr::{BoolOpKind, BoolOpSurface, Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// Past this many unrelated definers, `is_a?` arms stop being an
/// improvement on the nil guard and start being noise. Nothing in the
/// corpus reaches it — the widest real set (`to_gid_param`, on every
/// model) collapses to ONE arm through its common base.
const MAX_ARMS: usize = 3;

pub fn apply_try_guard_lowering(app: &mut App) {
    let definers = collect_definers(app);
    let parents = collect_parents(app);
    super::for_each_hook_body(app, &mut |e| rewrite(e, &definers, &parents));
    // THE TEST TREE TOO, which `for_each_hook_body` does not reach — and
    // for this pass that is where the whole demand is. campfire's single
    // `try` site is in `turbo_test_helper.rb`, spliced into every test
    // class that asserts a broadcast; the app tree has none at all.
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &definers, &parents);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &definers, &parents);
        }
        for h in &mut tm.helpers {
            rewrite(&mut h.body, &definers, &parents);
        }
        for (_, value) in &mut tm.constants {
            rewrite(value, &definers, &parents);
        }
        for ic in &mut tm.inner_classes {
            for m in &mut ic.methods {
                rewrite(&mut m.body, &definers, &parents);
            }
        }
    }
}

fn rewrite(
    expr: &mut Expr,
    definers: &HashMap<Symbol, HashSet<Symbol>>,
    parents: &HashMap<Symbol, Symbol>,
) {
    expr.node.for_each_child_mut(&mut |c| rewrite(c, definers, parents));

    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*expr.node else {
        return;
    };
    if method.as_str() != "try" && method.as_str() != "try!" {
        return;
    }
    // `try!` raises where `try` returns nil, so it is ALREADY the nil
    // guard's semantics for a missing method — leave it to the desugar
    // below rather than narrowing it into a silent nil.
    let bang = method.as_str() == "try!";
    if block.is_some() {
        return;
    }
    let Some(name) = args.first().and_then(literal_name) else { return };

    let recv = recv.clone();
    let rest: Vec<Expr> = args.iter().skip(1).cloned().collect();
    let span = expr.span;
    let call = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: name.clone(),
            args: rest,
            block: None,
            parenthesized: true,
        },
    );

    let arms = if bang { None } else { narrowing(&name, definers, parents) };
    *expr = match arms {
        // Nothing in the tree defines it and it is not `try!`: the only
        // answer `try` can give is nil. Folding rather than emitting a
        // call that cannot resolve — the same rule the rest of the
        // pipeline follows for a send with no receiver to dispatch on.
        Some(arms) if arms.is_empty() => Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
        Some(arms) => {
            let test = arms
                .into_iter()
                .map(|klass| is_a(span, &recv, &klass))
                .reduce(|l, r| {
                    Expr::new(
                        span,
                        ExprNode::BoolOp {
                            op: BoolOpKind::Or,
                            surface: BoolOpSurface::Symbol,
                            left: l,
                            right: r,
                        },
                    )
                })
                .expect("non-empty");
            Expr::new(
                span,
                ExprNode::If {
                    cond: test,
                    then_branch: call,
                    else_branch: Expr::new(span, ExprNode::Lit { value: Literal::Nil }),
                },
            )
        }
        // Unknown, or `try!`. The nil guard is what this has always
        // emitted and it is right whenever the receiver is nil or does
        // define the method.
        None => Expr::new(
            span,
            ExprNode::BoolOp {
                op: BoolOpKind::And,
                surface: BoolOpSurface::Symbol,
                left: recv,
                right: call,
            },
        ),
    };
}

/// The classes to test with `is_a?`, or None to keep the nil guard.
///
/// `Some(vec![])` means NOTHING defines the name — a distinct answer
/// from "give up", and the one that folds to nil.
fn narrowing(
    name: &Symbol,
    definers: &HashMap<Symbol, HashSet<Symbol>>,
    parents: &HashMap<Symbol, Symbol>,
) -> Option<Vec<Symbol>> {
    let Some(set) = definers.get(name) else {
        // The name is defined nowhere in the tree. It may still be a
        // RUNTIME method (`to_param`, `strip`) — this pass only sees app
        // classes — so folding to nil here would be wrong for exactly
        // the methods the runtime supplies. Keep the guard.
        return None;
    };
    if set.is_empty() {
        return None;
    }
    // COVER the set with as few `is_a?` tests as possible: for each
    // definer, climb to the HIGHEST ancestor all of whose descendants
    // also define the name, and dedupe.
    //
    // One base is the common case and not the only one. campfire's
    // `to_gid_param` is on all fifteen models, thirteen of which
    // descend from `ApplicationRecord` — and `Opengraph::Location` and
    // `Opengraph::Metadata` descend from nothing at all, so a
    // single-base rule found no ancestor and gave up on a set it could
    // have covered in three tests.
    //
    // Climbing is what keeps the test from being too WIDE:
    // `ApplicationRecord` is right for `to_gid_param` and would be
    // badly wrong for `username`, where Room descends from the same
    // base and answers nothing.
    let mut cover: Vec<Symbol> = Vec::new();
    for klass in set {
        let base = highest_safe_ancestor(klass, set, parents);
        if !cover.contains(&base) {
            cover.push(base);
        }
    }
    if cover.len() > MAX_ARMS {
        return None;
    }
    // Deterministic emit: a HashSet's iteration order is not.
    cover.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Some(cover)
}

/// The furthest ancestor of `klass` that is still SAFE to test — every
/// class in the tree descending from it defines the name too. Falls back
/// to `klass` itself, which is always safe.
fn highest_safe_ancestor(
    klass: &Symbol,
    set: &HashSet<Symbol>,
    parents: &HashMap<Symbol, Symbol>,
) -> Symbol {
    let mut best = klass.clone();
    for ancestor in ancestry(klass, parents) {
        if set.contains(&ancestor) && descendants_all_define(&ancestor, set, parents) {
            best = ancestor;
        }
    }
    best
}

/// `[self, parent, grandparent, …]`, stopping at a name the tree does
/// not define (`ActiveRecord::Base` is the runtime's, not an app
/// class, so the chain ends before it).
fn ancestry(name: &Symbol, parents: &HashMap<Symbol, Symbol>) -> Vec<Symbol> {
    let mut out = vec![name.clone()];
    let mut cur = name.clone();
    // A malformed tree with a parent cycle must not hang the compiler.
    let mut seen = HashSet::new();
    while let Some(p) = parents.get(&cur) {
        if !seen.insert(p.clone()) {
            break;
        }
        out.push(p.clone());
        cur = p.clone();
    }
    out
}

/// Does every class under `base` ANSWER the name — by defining it or by
/// inheriting it from something that does?
///
/// RESPONDS, not DEFINES, and the distinction is the whole of STI.
/// `to_gid_param` is synthesized on `Room` and not on `Rooms::Open`,
/// which inherits it; asking about definition alone made
/// `ApplicationRecord` look unsafe, nothing climbed, and a set that
/// covers in three tests came out as fifteen.
fn descendants_all_define(
    base: &Symbol,
    set: &HashSet<Symbol>,
    parents: &HashMap<Symbol, Symbol>,
) -> bool {
    parents
        .keys()
        .filter(|k| ancestry(k, parents).contains(base))
        .all(|k| k == base || ancestry(k, parents).iter().any(|a| set.contains(a)))
}

fn is_a(span: crate::span::Span, recv: &Expr, klass: &Symbol) -> Expr {
    let path: Vec<Symbol> = klass.as_str().split("::").map(Symbol::from).collect();
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv.clone()),
            method: Symbol::from("is_a?"),
            args: vec![Expr::new(span, ExprNode::Const { path })],
            block: None,
            parenthesized: true,
        },
    )
}

/// Methods the pipeline SYNTHESIZES onto every model, which this pass
/// cannot see because it runs first.
///
/// `to_gid_param` is pushed by `lower::broadcasts::push_to_gid_param`
/// from `model_to_library`, i.e. at emit-prep time, well after the
/// post-analyze passes. Without this the one `try` site in the corpus
/// reads as "nothing defines that name" and keeps the nil guard — which
/// is the bug.
///
/// A second copy of a one-item list, and it is guarded the way
/// `RUNTIME_MIXIN_TARGETS` is: `tests/try_guard.rs` fails if
/// `lower::broadcasts` stops synthesizing it. Adding an entry here
/// without the method actually being universal would narrow a `try` to
/// a class that does not answer, so the guard is not optional.
const SYNTHESIZED_ON_EVERY_MODEL: [&str; 1] = ["to_gid_param"];

/// method name -> the app classes defining it as an INSTANCE method.
fn collect_definers(app: &App) -> HashMap<Symbol, HashSet<Symbol>> {
    let mut out: HashMap<Symbol, HashSet<Symbol>> = HashMap::new();
    for m in &app.models {
        let owner = m.name.0.clone();
        for name in SYNTHESIZED_ON_EVERY_MODEL {
            out.entry(Symbol::from(name)).or_default().insert(owner.clone());
        }
        for item in &m.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                if matches!(method.receiver, MethodReceiver::Instance) {
                    out.entry(method.name.clone()).or_default().insert(owner.clone());
                }
            }
        }
    }
    for lc in &app.library_classes {
        let owner = lc.name.0.clone();
        for method in &lc.methods {
            if matches!(method.receiver, MethodReceiver::Instance) {
                out.entry(method.name.clone()).or_default().insert(owner.clone());
            }
        }
    }
    out
}

/// class -> superclass, over models and library classes both.
fn collect_parents(app: &App) -> HashMap<Symbol, Symbol> {
    let mut out = HashMap::new();
    for m in &app.models {
        if let Some(p) = &m.parent {
            out.insert(m.name.0.clone(), p.0.clone());
        }
    }
    for lc in &app.library_classes {
        if let Some(p) = &lc.parent {
            out.insert(lc.name.0.clone(), p.0.clone());
        }
    }
    out
}

fn literal_name(e: &Expr) -> Option<Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(Symbol::from(value.as_str())),
        _ => None,
    }
}
