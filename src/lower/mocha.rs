//! mocha, lowered to a typed stub slot.
//!
//! `Resolv.stubs(:getaddresses).with("h").returns([ip])` is a runtime
//! method-table swap. There is no method table to swap on a strict
//! target, and mocha is a Ruby metaprogramming library with nothing to
//! compile against — so campfire's stubbing tests were a PERMANENT
//! ceiling, ledgered as "nothing to fix until those gems have a spinel
//! surface".
//!
//! They are not, because the set of stub sites is CLOSED at transpile
//! time. This is the same argument that licensed monomorphizing
//! `lower::class_body_new`: an ingested tree's stubs are all visible
//! here, so a copy-and-bind is available where late binding is not.
//!
//! WHAT THE CORPUS ACTUALLY ASKS FOR. Measured across campfire's suite
//! before designing anything — 43 sites, and 41 of 42 stub a CLASS-SIDE
//! method on a statically-known constant:
//!
//! ```text
//!   Resolv.getaddresses            13     WebPush.payload_send        11
//!   TCPSocket.open                  8     SecureRandom.alphanumeric    4
//!   Turbo::StreamsChannel.*         4     Random.uuid                  1
//!   Webhook.any_instance.post       1   (the only instance-level one)
//! ```
//!
//! So the lowering does not need a general mocha. It needs a rule table
//! keyed by (constant, method), which is what `STUBBABLE` is — adding
//! `WebPush.payload_send` is a row plus a slot on the facade, not new
//! code here.
//!
//! WHY A FACADE SLOT RATHER THAN `self.new`-STYLE INDIRECTION. Most of
//! these are methods on OUR OWN runtime shims, and `GemFacade.fail!` is
//! already the single point where "there is no real implementation"
//! gets decided. A stub slot belongs exactly there: with one installed
//! the facade answers it, with none it fails as loudly as before. The
//! seam costs one comparison on a table seeded with a sentinel.
//!
//! IT RUNS ON EVERY TARGET, INCLUDING CRuby. A lowering may not branch
//! on the target, so the ruby family gets the same rewrite — and
//! therefore the same slot, bolted onto the stdlib class by
//! `project::RESOLV_STUB_REOPEN`. That is a deliberate trade: those
//! sites stop exercising real mocha under CRuby and start exercising
//! the same slot every other target uses, which is the parity this
//! pipeline is for.
//!
//! WHAT IS DELIBERATELY LEFT ALONE. A chain this pass does not fully
//! understand is not rewritten, so it still fails loudly at the `stubs`
//! call rather than being silently dropped — the failure mode that
//! matters is a stub that never took while the test reports green.
//! `expects` (verification), `.raises`, `.times`/`.never`/`.once`,
//! `any_instance`, and the structural matchers (`has_entry`,
//! `hash_including`) are all still ceiling; each wants its own slot
//! shape and its own teardown assertion.

use crate::app::App;
use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

/// (constant, stubbed method, keyed setter, catch-all setter).
///
/// One row per method that has a slot. The table is the whole
/// registration surface: a method absent from it keeps its mocha
/// spelling and its loud failure.
///
/// Two setters because mocha has two shapes and they answer different
/// questions — `.with(h).returns(v)` files an answer under `h`, and a
/// bare `.returns(v)` answers for every argument.
const STUBBABLE: &[(&str, &str, &str, &str)] =
    &[("Resolv", "getaddresses", "stub_getaddresses", "stub_getaddresses_any")];

pub fn apply_mocha_lowering(app: &mut App) {
    for tm in &mut app.test_modules {
        if let Some(setup) = &mut tm.setup {
            rewrite(setup);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body);
        }
    }
}

/// The setter pair for a `<Const>.stubs(:method)` head, if it has one.
fn setter_for(recv: &Expr, method: &Symbol, args: &[Expr]) -> Option<(&'static str, &'static str)> {
    if method.as_str() != "stubs" || args.len() != 1 {
        return None;
    }
    let ExprNode::Const { path } = &*recv.node else { return None };
    // Match on the LAST segment, so `::Resolv` and `Resolv` both hit.
    let konst = path.last()?.as_str();
    let ExprNode::Lit { value: crate::expr::Literal::Sym { value: stubbed } } = &*args[0].node
    else {
        return None;
    };
    STUBBABLE
        .iter()
        .find(|(k, m, _, _)| *k == konst && *m == stubbed.as_str())
        .map(|(_, _, keyed, any)| (*keyed, *any))
}

/// `<Const>.stubs(:m).with(a).returns(v)` -> `<Const>.<setter>(a, v)`.
///
/// Peels outside-in, because that is how the chain nests: `returns` is
/// the outermost send and `stubs` the innermost receiver.
fn rewrite(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut rewrite);

    let ExprNode::Send { recv: Some(outer), method, args, .. } = &*expr.node else { return };
    // `returns` with more than one argument is a SEQUENCE — first call
    // gets the first value, second the second. A single slot cannot say
    // that, so leave it mocha-spelled and loudly broken rather than
    // answer the first value forever.
    if method.as_str() != "returns" || args.len() != 1 {
        return;
    }
    let returned = args[0].clone();

    // Either `<Const>.stubs(:m)` directly (catch-all), or
    // `<Const>.stubs(:m).with(a)` (keyed on `a`).
    let (konst, setter, mut call_args) = match &*outer.node {
        ExprNode::Send { recv: Some(head), method: with_m, args: with_args, .. }
            if with_m.as_str() == "with" && with_args.len() == 1 =>
        {
            let ExprNode::Send { recv: Some(k), method: sm, args: sa, .. } = &*head.node else {
                return;
            };
            let Some((keyed, _)) = setter_for(k, sm, sa) else { return };
            (k.clone(), keyed, vec![with_args[0].clone()])
        }
        ExprNode::Send { recv: Some(k), method: sm, args: sa, .. } => {
            let Some((_, any)) = setter_for(k, sm, sa) else { return };
            (k.clone(), any, vec![])
        }
        _ => return,
    };
    call_args.push(returned);

    let span = expr.span;
    *expr = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(konst),
            method: Symbol::from(setter),
            args: call_args,
            block: None,
            parenthesized: true,
        },
    );
}
