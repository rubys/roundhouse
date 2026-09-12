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
//! WHAT A CHAIN THIS PASS DOES NOT SERVE BECOMES. It used to be left
//! spelled as mocha, which is loud in the right place under CRuby (real
//! mocha) and loud in the WRONG place on a strict target: `stubs` is a
//! frontend refusal there, so one unserved chain took its whole FILE off
//! the compiled lane — `push_subscription_test`, 17 tests, for one
//! `.with { |*| … }` block predicate most of them never reach. Now every
//! recognised chain the table cannot serve is handed to `MochaBridge.
//! chain(...)` AS DATA: the constant by name, the kind, the method, the
//! ops list, and the `with` block if there was one. The file that
//! defines the bridge is per target (`project.rs`): the ruby family
//! REPLAYS it through the real gem, so nothing those lanes passed
//! changes; a strict target raises, per test, at the call. Same for a
//! bare structural matcher (`has_entry(...)`) — `MochaBridge.matcher`.
//!
//! `expects` is served for the rows that declare a count slot:
//! `.never`/`.once`/`.twice`/`.times(n)` (and a bare `expects`, which
//! mocha reads as once) become `<Const>.expect_<m>(n)`, and the helper's
//! teardown runs each row's verify, which raises on a mismatch the way
//! `mocha_verify` does — a failure of THAT test.

use crate::app::App;
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;

/// One stubbable method, and everything the emit needs to serve it.
///
/// The table is the whole registration surface: a (constant, method)
/// absent from it, or a chain shape a row has no setter for, goes to
/// the bridge.
struct Stubbable {
    konst: &'static str,
    method: &'static str,
    /// `.with(a).returns(v)` — files an answer under `a`.
    keyed: Option<&'static str>,
    /// A bare `.returns(v)` — answers for every argument.
    any: Option<&'static str>,
    /// `stubs(:m)` with nothing chained — answers the row's default.
    bare: Option<&'static str>,
    /// `stubs(:m).raises(e)` — every call raises `e`.
    raises: Option<&'static str>,
    /// `stubs(:m).with { |*| … }.returns(v)` — answers `v` for the
    /// calls the block admits; the block rides on the slot call.
    where_: Option<&'static str>,
    /// `expects(:m).times(n)` — installs the stub and files a count the
    /// row's `verify` checks at teardown.
    expect: Option<&'static str>,
    /// `expects(:m)[.times(n)].with(has_entry(k: v))` — files the count
    /// against calls carrying that option, as `(n, :k, v)`.
    expect_entry: Option<&'static str>,
    /// Drops every installed stub and count; the helper runs this in
    /// setup, between tests.
    clear: &'static str,
    /// Raises when a filed count was not met; the helper runs this in
    /// teardown. Rows sharing a slot share the name and it is run once.
    verify: Option<&'static str>,
    /// The runtime file that DEFINES the slot. The helper requires it
    /// so the clear call is unconditional — see `stub_preamble`.
    require: &'static str,
    /// The constant the slot lives on, when it is not `konst`: the
    /// test stubs the name the app WROTE, and the slot sits where the
    /// app's call was grounded to (`Random.uuid` → `SecureRandom.uuid`,
    /// `lower::random_formatter`). `None` means `konst`.
    slot_on: Option<&'static str>,
}

impl Stubbable {
    /// The constant the slot call and the helper's clear/verify go to.
    fn slot_konst(&self) -> &'static str {
        self.slot_on.unwrap_or(self.konst)
    }
}

const STUBBABLE: &[Stubbable] = &[
    Stubbable {
        konst: "Resolv",
        method: "getaddresses",
        keyed: Some("stub_getaddresses"),
        any: Some("stub_getaddresses_any"),
        bare: None,
        raises: Some("stub_getaddresses_raises"),
        where_: Some("stub_getaddresses_where"),
        expect: None,
        expect_entry: None,
        clear: "clear_getaddresses_stubs",
        verify: None,
        require: "../runtime/resolv",
        slot_on: None,
    },
    // The stdlib's CSPRNG (spinel: `packages/securerandom`). campfire's
    // bot and user tests pin `alphanumeric` / `uuid` to a literal and
    // assert the key or address minted from it. The slot is a reopen on
    // both families — see `runtime/spinel/secure_random_stub.rb` and
    // `project::SECURE_RANDOM_STUB_REOPEN`.
    Stubbable {
        konst: "SecureRandom",
        method: "alphanumeric",
        keyed: None,
        any: Some("stub_alphanumeric"),
        bare: None,
        raises: None,
        where_: None,
        expect: None,
        expect_entry: None,
        clear: "clear_secure_random_stubs",
        verify: None,
        require: "../runtime/secure_random_stub",
        slot_on: None,
    },
    Stubbable {
        konst: "SecureRandom",
        method: "uuid",
        keyed: None,
        any: Some("stub_uuid"),
        bare: None,
        raises: None,
        where_: None,
        expect: None,
        expect_entry: None,
        clear: "clear_secure_random_stubs",
        verify: None,
        require: "../runtime/secure_random_stub",
        slot_on: None,
    },
    // `Random.uuid` is `SecureRandom.uuid` once lowered (the app's
    // call and the test's stub have to meet at ONE method), so the
    // slot is SecureRandom's.
    Stubbable {
        konst: "Random",
        method: "uuid",
        keyed: None,
        any: Some("stub_uuid"),
        bare: None,
        raises: None,
        where_: None,
        expect: None,
        expect_entry: None,
        clear: "clear_secure_random_stubs",
        verify: None,
        require: "../runtime/secure_random_stub",
        slot_on: Some("SecureRandom"),
    },
    // The façade for the `web-push` gem. A bare `stubs` answers `""`,
    // which is what a test that only wants delivery to not happen
    // needs; `expects(...).never` / `.times(n)` are the shapes
    // campfire's push tests write.
    Stubbable {
        konst: "WebPush",
        method: "payload_send",
        keyed: None,
        any: Some("stub_payload_send_any"),
        bare: Some("stub_payload_send"),
        raises: None,
        where_: None,
        expect: Some("expect_payload_send"),
        expect_entry: Some("expect_payload_send_with_entry"),
        clear: "clear_payload_send_stubs",
        verify: Some("verify_payload_send_expectations"),
        require: "../runtime/gem_facades",
        slot_on: None,
    },
    // Our own channel class (`runtime/spinel/turbo_streams.rb`, every
    // ruby-family tree carries it). campfire's messages controller
    // tests expect one replace / one remove per update / destroy.
    Stubbable {
        konst: "Turbo::StreamsChannel",
        method: "broadcast_replace_to",
        keyed: None,
        any: None,
        bare: None,
        raises: None,
        where_: None,
        expect: Some("expect_broadcast_replace_to"),
        expect_entry: None,
        clear: "clear_broadcast_expectations",
        verify: Some("verify_broadcast_expectations"),
        require: "../runtime/turbo_streams",
        slot_on: None,
    },
    Stubbable {
        konst: "Turbo::StreamsChannel",
        method: "broadcast_remove_to",
        keyed: None,
        any: None,
        bare: None,
        raises: None,
        where_: None,
        expect: Some("expect_broadcast_remove_to"),
        expect_entry: None,
        clear: "clear_broadcast_expectations",
        verify: Some("verify_broadcast_expectations"),
        require: "../runtime/turbo_streams",
        slot_on: None,
    },
];

/// The file that defines `MochaBridge` — per target, see `project.rs`.
const BRIDGE_REQUIRE: &str = "../runtime/mocha_bridge";

/// The chain methods mocha's expectation API answers. A method outside
/// this set ends the chain: the expression is not a stub and is left
/// alone.
const OPS: &[&str] = &[
    "with",
    "returns",
    "raises",
    "throws",
    "never",
    "once",
    "twice",
    "times",
    "at_least",
    "at_least_once",
    "at_most",
    "at_most_once",
    "then",
    "in_sequence",
    "yields",
    "multiple_yields",
];

/// mocha's bare parameter matchers (`Mocha::ParameterMatchers`), which
/// a test body calls as if they were its own methods. They are not, on
/// a strict target — so they travel to the bridge by name.
const MATCHERS: &[&str] = &[
    "anything",
    "any_parameters",
    "has_entry",
    "has_entries",
    "has_key",
    "has_value",
    "hash_including",
    "includes",
    "instance_of",
    "is_a",
    "kind_of",
    "regexp_matches",
    "equals",
    "optionally",
    "all_of",
    "any_of",
    "responds_with",
];

/// `require_relative` lines for every runtime file that defines a slot,
/// plus the bridge.
///
/// The helper REQUIRES them rather than guarding the clear call with
/// `defined?`, and that is not a style choice. On CRuby the stdlib
/// `Resolv` is already loaded (net/http reaches it), so
/// `defined?(Resolv)` is TRUE in a test file that never required our
/// port — and the guard sailed through to a `NoMethodError` on
/// `clear_getaddresses_stubs` in the setup of EVERY test. campfire's
/// CRuby conformance went 255/288 to 34/288 on exactly that.
///
/// Requiring the file makes the method's existence a fact rather than a
/// question. `require_relative`, because the helper's own note says the
/// AOT model follows only static `require_relative` chains.
pub fn stub_requires() -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = String::new();
    for s in STUBBABLE.iter().map(|s| s.require).chain(std::iter::once(BRIDGE_REQUIRE)) {
        if seen.contains(&s) {
            continue;
        }
        seen.push(s);
        out.push_str(&format!("require_relative \"{s}\"\n"));
    }
    out
}

/// The `clear` call for every slot, one per line, for the helper to run
/// between tests.
///
/// mocha unstubs in its own teardown, and a slot that does not is WORSE
/// than no slot: a stub installed by one test answers for every later
/// one in the file. Not hypothetical — shipping this without a clear
/// took `opengraph_location_test` from 7/7 to 6/7 under CRuby, and the
/// test it broke (`test_read_valid_html`) does not stub DNS at all. It
/// inherited the previous test's answer.
pub fn stub_clear_lines(indent: &str) -> String {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    let mut out = String::new();
    for s in STUBBABLE {
        let konst = s.slot_konst();
        if seen.contains(&(konst, s.clear)) {
            continue;
        }
        seen.push((konst, s.clear));
        out.push_str(&format!("{indent}{}.{}\n", konst, s.clear));
    }
    out
}

/// The `verify` call for every slot that files counts, for the helper's
/// teardown. Each raises on an unmet count — the same moment, and the
/// same failure, as `mocha_verify`.
pub fn stub_verify_lines(indent: &str) -> String {
    let mut seen: Vec<(&str, &str)> = Vec::new();
    let mut out = String::new();
    for s in STUBBABLE {
        let Some(verify) = s.verify else { continue };
        let konst = s.slot_konst();
        if seen.contains(&(konst, verify)) {
            continue;
        }
        seen.push((konst, verify));
        out.push_str(&format!("{indent}{}.{}\n", konst, verify));
    }
    out
}

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

/// One link of an expectation chain: `.with(a)`, `.returns(v)`,
/// `.times(n)`, `.with { … }`.
struct Op {
    name: Symbol,
    args: Vec<Expr>,
    block: Option<Expr>,
}

/// A recognised chain, head to tail: `<konst>[.any_instance].<kind>(:<method>)` then `ops`.
struct Chain {
    konst: Expr,
    /// The constant as spelled, `::`-joined.
    path: String,
    any_instance: bool,
    /// `stubs` or `expects`.
    kind: Symbol,
    method: Symbol,
    /// In source order, innermost first.
    ops: Vec<Op>,
}

/// Read a chain off `expr`, walking receivers inward until the
/// `stubs`/`expects` head. Anything else along the way — a method that
/// is not one of mocha's, a head whose receiver is not a constant, a
/// non-Symbol method name — means this is not a stub, and `None` leaves
/// the expression alone.
fn parse_chain(expr: &Expr) -> Option<Chain> {
    let mut ops: Vec<Op> = Vec::new();
    let mut cur = expr;
    loop {
        let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*cur.node else {
            return None;
        };
        let m = method.as_str();
        if (m == "stubs" || m == "expects") && args.len() == 1 && block.is_none() {
            let ExprNode::Lit { value: Literal::Sym { value: stubbed } } = &*args[0].node else {
                return None;
            };
            let (konst, any_instance) = match &*recv.node {
                ExprNode::Const { .. } => (recv.clone(), false),
                ExprNode::Send { recv: Some(k), method: ai, args: a, block: None, .. }
                    if ai.as_str() == "any_instance"
                        && a.is_empty()
                        && matches!(&*k.node, ExprNode::Const { .. }) =>
                {
                    (k.clone(), true)
                }
                _ => return None,
            };
            let ExprNode::Const { path } = &*konst.node else { return None };
            let path = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            ops.reverse();
            return Some(Chain { konst, path, any_instance, kind: method.clone(), method: stubbed.clone(), ops });
        }
        if !OPS.contains(&m) {
            return None;
        }
        ops.push(Op { name: method.clone(), args: args.clone(), block: block.clone() });
        cur = recv;
    }
}

/// The row for a chain's head, if the table has one. A row spelled
/// without `::` matches on the constant's last segment (`::Resolv` and
/// `Resolv` both hit); one spelled with it wants the whole path.
fn row_for(path: &str, method: &str) -> Option<&'static Stubbable> {
    let last = path.rsplit("::").next().unwrap_or(path);
    STUBBABLE.iter().find(|s| {
        s.method == method
            && if s.konst.contains("::") { s.konst == path || path.ends_with(&format!("::{}", s.konst)) } else { s.konst == last }
    })
}

/// Top-down, and that direction is load-bearing: a chain is one
/// expression and has to be read whole. The bottom-up walk this used to
/// do would have rewritten (or bridged) the inner `stubs(:m)` before the
/// outer `.returns(v)` was ever seen.
fn rewrite(expr: &mut Expr) {
    if let Some(chain) = parse_chain(expr) {
        let span = expr.span;
        *expr = lower_chain(span, chain);
        return;
    }
    expr.node.for_each_child_mut(&mut rewrite);
}

fn lower_chain(span: crate::span::Span, chain: Chain) -> Expr {
    if !chain.any_instance {
        if let Some(row) = row_for(&chain.path, chain.method.as_str()) {
            if let Some(served) = lower_known(span, &chain, row) {
                return served;
            }
        }
    }
    bridge_chain(span, chain)
}

fn int_lit(span: crate::span::Span, v: i64) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Int { value: v } })
}

fn call(span: crate::span::Span, recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
    Expr::new(
        span,
        ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized: true },
    )
}

/// The count an `expects` link files, if it is one of the counting
/// links: `never` 0, `once` 1, `twice` 2, `times(n)` n.
fn count_of(op: &Op) -> Option<Expr> {
    if op.block.is_some() {
        return None;
    }
    match (op.name.as_str(), op.args.as_slice()) {
        ("never", []) => Some(int_lit(Span_of(op), 0)),
        ("once", []) => Some(int_lit(Span_of(op), 1)),
        ("twice", []) => Some(int_lit(Span_of(op), 2)),
        ("times", [n]) => Some(n.clone()),
        _ => None,
    }
}

#[allow(non_snake_case)]
fn Span_of(op: &Op) -> crate::span::Span {
    op.args.first().map(|a| a.span).unwrap_or_else(crate::span::Span::synthetic)
}

/// A chain the row can serve, as the slot call — or `None`, and the
/// bridge takes it.
fn lower_known(span: crate::span::Span, chain: &Chain, row: &Stubbable) -> Option<Expr> {
    let konst = match row.slot_on {
        Some(on) => Expr::new(chain.konst.span, ExprNode::Const { path: vec![Symbol::from(on)] }),
        None => chain.konst.clone(),
    };
    let ops = &chain.ops;
    let plain = |op: &Op, name: &str, arity: usize| op.name.as_str() == name && op.args.len() == arity && op.block.is_none();
    match chain.kind.as_str() {
        "stubs" => match ops.as_slice() {
            [] => row.bare.map(|bare| call(span, konst, bare, vec![])),
            [ret] if plain(ret, "returns", 1) => row.any.map(|any| call(span, konst, any, vec![ret.args[0].clone()])),
            [raises] if plain(raises, "raises", 1) => {
                row.raises.map(|slot| call(span, konst, slot, vec![raises.args[0].clone()]))
            }
            [with, ret] if plain(with, "with", 1) && plain(ret, "returns", 1) => {
                row.keyed.map(|keyed| call(span, konst, keyed, vec![with.args[0].clone(), ret.args[0].clone()]))
            }
            // `.with { |*| … }.returns(v)`: the block is the predicate,
            // handed to the slot as a one-parameter LAMBDA (the host) —
            // a value the runtime's `Array[^(String) -> bool]` can hold,
            // where a `&blk` could not be typed.
            [with, ret] if with.name.as_str() == "with" && with.args.is_empty() && with.block.is_some() && plain(ret, "returns", 1) => {
                let slot = row.where_?;
                let pred = predicate_lambda(with.block.clone()?)?;
                Some(call(span, konst, slot, vec![ret.args[0].clone(), pred]))
            }
            _ => None,
        },
        "expects" => {
            // A `.with(has_entry(k: v))` link beside at most one count.
            let entries: Vec<&Op> = ops.iter().filter(|op| op.name.as_str() == "with").collect();
            let counts: Vec<&Op> = ops.iter().filter(|op| op.name.as_str() != "with").collect();
            let n = match counts.as_slice() {
                // mocha reads a bare `expects` as exactly once.
                [] => int_lit(span, 1),
                [count] => count_of(count)?,
                _ => return None,
            };
            match entries.as_slice() {
                [] => Some(call(span, konst, row.expect?, vec![n])),
                [with] if plain(with, "with", 1) => {
                    let (key, value) = has_entry_pair(&with.args[0])?;
                    Some(call(span, konst, row.expect_entry?, vec![n, sym_lit(span, key.as_str()), value]))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// A `with` block as a lambda over the ONE argument the slot passes
/// (the host). `|*|` / no params take a placeholder so the lambda's
/// arity matches; a block naming its own parameters is left as it is
/// (mocha would hand it every argument, and this slot has one).
fn predicate_lambda(block: Expr) -> Option<Expr> {
    let ExprNode::Lambda { params, rest_param, block_param, body, block_style } = *block.node else {
        return None;
    };
    let params = if params.is_empty() && rest_param.is_none() { vec![Symbol::from("_host")] } else { params };
    Some(Expr::new(
        block.span,
        ExprNode::Lambda { params, rest_param, block_param, body, block_style },
    ))
}

/// `has_entry(k: v)` with exactly one Symbol-keyed pair — the one
/// matcher shape a slot can hold as `(key, value)`. Anything else
/// (two pairs, a non-Symbol key, another matcher) goes to the bridge.
fn has_entry_pair(arg: &Expr) -> Option<(Symbol, Expr)> {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*arg.node else { return None };
    if method.as_str() != "has_entry" || args.len() != 1 {
        return None;
    }
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
    let [(k, v)] = entries.as_slice() else { return None };
    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
    Some((key.clone(), v.clone()))
}

fn array_lit(span: crate::span::Span, elems: Vec<Expr>) -> Expr {
    Expr::new(span, ExprNode::Array { elements: elems, style: crate::expr::ArrayStyle::default() })
}

fn sym_lit(span: crate::span::Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(s) } })
}

fn str_lit(span: crate::span::Span, s: &str) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: s.to_string() } })
}

fn bridge() -> Expr {
    Expr::new(crate::span::Span::synthetic(), ExprNode::Const { path: vec![Symbol::from("MochaBridge")] })
}

/// One argument of a chain link, as the bridge wants it. A constant
/// travels by NAME (`raises(Net::OpenTimeout)`): a class as a value is
/// not a shape every target carries, and the replay side resolves the
/// name. A bare matcher call becomes `MochaBridge.matcher(:name, arg)`.
fn bridge_arg(arg: Expr) -> Expr {
    let span = arg.span;
    match &*arg.node {
        ExprNode::Const { path } => str_lit(span, &path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")),
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if MATCHERS.contains(&method.as_str()) && args.len() <= 1 =>
        {
            let mut margs = vec![sym_lit(span, method.as_str())];
            margs.extend(args.iter().cloned().map(bridge_arg));
            call(span, bridge(), "matcher", margs)
        }
        _ => arg,
    }
}

/// `MochaBridge.chain("<Const>", :<kind>, :<method>, [[:op, [args…]], …]) { with-block }`.
fn bridge_chain(span: crate::span::Span, chain: Chain) -> Expr {
    let mut block: Option<Expr> = None;
    let mut links: Vec<Expr> = Vec::new();
    for op in chain.ops {
        let args: Vec<Expr> = op.args.into_iter().map(bridge_arg).collect();
        if op.block.is_some() {
            block = op.block;
        }
        links.push(array_lit(span, vec![sym_lit(span, op.name.as_str()), array_lit(span, args)]));
    }
    let kind = if chain.any_instance { format!("any_instance_{}", chain.kind.as_str()) } else { chain.kind.as_str().to_string() };
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(bridge()),
            method: Symbol::from("chain"),
            args: vec![
                str_lit(span, &chain.path),
                sym_lit(span, &kind),
                sym_lit(span, chain.method.as_str()),
                array_lit(span, links),
            ],
            block,
            parenthesized: true,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn sp() -> Span {
        Span::synthetic()
    }
    fn sym(s: &str) -> Expr {
        sym_lit(sp(), s)
    }
    fn konst(path: &[&str]) -> Expr {
        Expr::new(sp(), ExprNode::Const { path: path.iter().map(|s| Symbol::from(*s)).collect() })
    }
    fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(sp(), ExprNode::Send { recv, method: Symbol::from(method), args, block: None, parenthesized: true })
    }
    fn send_blk(recv: Expr, method: &str, block: Expr) -> Expr {
        Expr::new(
            sp(),
            ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args: vec![], block: Some(block), parenthesized: false },
        )
    }
    fn lambda() -> Expr {
        Expr::new(
            sp(),
            ExprNode::Lambda {
                rest_param: None,
                params: vec![],
                block_param: None,
                body: Expr::new(sp(), ExprNode::Lit { value: Literal::Bool { value: true } }),
                block_style: crate::expr::BlockStyle::Brace,
            },
        )
    }
    fn as_send(e: &Expr) -> (&Expr, &str, &[Expr], &Option<Expr>) {
        let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*e.node else { panic!("{:?}", e.node) };
        (recv, method.as_str(), args, block)
    }
    fn const_path(e: &Expr) -> String {
        let ExprNode::Const { path } = &*e.node else { panic!("{:?}", e.node) };
        path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::")
    }
    fn int_of(e: &Expr) -> i64 {
        let ExprNode::Lit { value: Literal::Int { value } } = &*e.node else { panic!("{:?}", e.node) };
        *value
    }

    #[test]
    fn a_keyed_returns_still_lowers_to_the_typed_slot() {
        let mut e = send(
            Some(send(Some(send(Some(konst(&["Resolv"])), "stubs", vec![sym("getaddresses")])), "with", vec![str_lit(sp(), "h")])),
            "returns",
            vec![array_lit(sp(), vec![str_lit(sp(), "1.2.3.4")])],
        );
        rewrite(&mut e);
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "Resolv");
        assert_eq!(m, "stub_getaddresses");
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn a_bare_stubs_lowers_to_the_rows_default() {
        let mut e = send(Some(konst(&["WebPush"])), "stubs", vec![sym("payload_send")]);
        rewrite(&mut e);
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "WebPush");
        assert_eq!(m, "stub_payload_send");
        assert!(args.is_empty());
    }

    #[test]
    fn expects_with_a_count_files_the_count() {
        for (link, n) in [("never", 0), ("once", 1), ("twice", 2)] {
            let mut e = send(Some(send(Some(konst(&["WebPush"])), "expects", vec![sym("payload_send")])), link, vec![]);
            rewrite(&mut e);
            let (_, m, args, _) = as_send(&e);
            assert_eq!(m, "expect_payload_send", "{link}");
            assert_eq!(int_of(&args[0]), n, "{link}");
        }
        // `.times(n)` carries its argument; a bare `expects` is once.
        let mut e = send(
            Some(send(Some(konst(&["Turbo", "StreamsChannel"])), "expects", vec![sym("broadcast_remove_to")])),
            "times",
            vec![int_lit(sp(), 3)],
        );
        rewrite(&mut e);
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "Turbo::StreamsChannel");
        assert_eq!(m, "expect_broadcast_remove_to");
        assert_eq!(int_of(&args[0]), 3);
        let mut e = send(Some(konst(&["Turbo", "StreamsChannel"])), "expects", vec![sym("broadcast_replace_to")]);
        rewrite(&mut e);
        let (_, m, args, _) = as_send(&e);
        assert_eq!(m, "expect_broadcast_replace_to");
        assert_eq!(int_of(&args[0]), 1);
    }

    #[test]
    fn a_chain_the_table_cannot_serve_travels_to_the_bridge_as_data() {
        // Resolv.stubs(:getaddresses).with { … }.throws(:x) — the row
        // has no `throws` setter.
        let head = send(Some(konst(&["Resolv"])), "stubs", vec![sym("getaddresses")]);
        let with = send_blk(head, "with", lambda());
        let mut e = send(Some(with), "throws", vec![sym("x")]);
        rewrite(&mut e);
        let (recv, m, args, block) = as_send(&e);
        assert_eq!(const_path(recv), "MochaBridge");
        assert_eq!(m, "chain");
        assert!(block.is_some(), "the with-block rides on the bridge call");
        assert!(matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == "Resolv"));
        assert!(matches!(&*args[1].node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "stubs"));
        let ExprNode::Array { elements: elems, .. } = &*args[3].node else { panic!("{:?}", args[3].node) };
        assert_eq!(elems.len(), 2, "with, throws — in source order");
        let ExprNode::Array { elements: first, .. } = &*elems[0].node else { panic!() };
        assert!(matches!(&*first[0].node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "with"));
    }

    #[test]
    fn a_with_block_predicate_rides_on_the_where_slot() {
        // Resolv.stubs(:getaddresses).with { |*| … }.returns([ip])
        let head = send(Some(konst(&["Resolv"])), "stubs", vec![sym("getaddresses")]);
        let with = send_blk(head, "with", lambda());
        let mut e = send(Some(with), "returns", vec![array_lit(sp(), vec![str_lit(sp(), "1.2.3.4")])]);
        rewrite(&mut e);
        let (recv, m, args, block) = as_send(&e);
        assert_eq!(const_path(recv), "Resolv");
        assert_eq!(m, "stub_getaddresses_where");
        assert_eq!(args.len(), 2, "the answer, then the predicate");
        assert!(block.is_none(), "the predicate travels as an argument, not a block");
        assert!(matches!(&*args[1].node, ExprNode::Lambda { params, .. } if params.len() == 1));
    }

    #[test]
    fn raises_and_has_entry_are_served_where_the_row_has_a_slot() {
        // Resolv.stubs(:getaddresses).raises(error)
        let head = send(Some(konst(&["Resolv"])), "stubs", vec![sym("getaddresses")]);
        let mut e = send(Some(head), "raises", vec![Expr::new(sp(), ExprNode::Ivar { name: Symbol::from("error") })]);
        rewrite(&mut e);
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "Resolv");
        assert_eq!(m, "stub_getaddresses_raises");
        assert_eq!(args.len(), 1);

        // WebPush.expects(:payload_send).with(has_entry(endpoint_ip: ip)) — bare expects is once.
        let matcher = send(None, "has_entry", vec![Expr::new(sp(), ExprNode::Hash { entries: vec![(sym("endpoint_ip"), str_lit(sp(), "1.2.3.4"))], kwargs: true })]);
        let head = send(Some(konst(&["WebPush"])), "expects", vec![sym("payload_send")]);
        let mut e = send(Some(head), "with", vec![matcher]);
        rewrite(&mut e);
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "WebPush");
        assert_eq!(m, "expect_payload_send_with_entry");
        assert_eq!(int_of(&args[0]), 1);
        assert!(matches!(&*args[1].node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "endpoint_ip"));
        assert!(matches!(&*args[2].node, ExprNode::Lit { value: Literal::Str { value } } if value == "1.2.3.4"));
    }

    #[test]
    fn a_matcher_and_a_constant_travel_by_name() {
        // WebPush.expects(:payload_send).with(has_entry(endpoint_ip: ip)).raises(Net::OpenTimeout)
        let matcher = send(None, "has_entry", vec![Expr::new(sp(), ExprNode::Hash { entries: vec![(sym("endpoint_ip"), str_lit(sp(), "1.2.3.4"))], kwargs: true })]);
        let head = send(Some(konst(&["WebPush"])), "expects", vec![sym("payload_send")]);
        let with = send(Some(head), "with", vec![matcher]);
        let mut e = send(Some(with), "raises", vec![konst(&["Net", "OpenTimeout"])]);
        rewrite(&mut e);
        let (_, m, args, _) = as_send(&e);
        assert_eq!(m, "chain");
        let ExprNode::Array { elements: elems, .. } = &*args[3].node else { panic!() };
        let ExprNode::Array { elements: with_link, .. } = &*elems[0].node else { panic!() };
        let ExprNode::Array { elements: with_args, .. } = &*with_link[1].node else { panic!() };
        let (recv, mm, margs, _) = as_send(&with_args[0]);
        assert_eq!(const_path(recv), "MochaBridge");
        assert_eq!(mm, "matcher");
        assert!(matches!(&*margs[0].node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "has_entry"));
        let ExprNode::Array { elements: raises_link, .. } = &*elems[1].node else { panic!() };
        let ExprNode::Array { elements: raises_args, .. } = &*raises_link[1].node else { panic!() };
        assert!(matches!(&*raises_args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == "Net::OpenTimeout"));
    }

    #[test]
    fn a_send_that_is_not_a_chain_is_left_alone() {
        let mut e = send(Some(konst(&["Resolv"])), "getaddresses", vec![str_lit(sp(), "h")]);
        let before = format!("{:?}", e);
        rewrite(&mut e);
        assert_eq!(format!("{:?}", e), before);
    }
}
