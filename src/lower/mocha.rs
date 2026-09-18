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
//!
//! THE APP'S OWN METHODS are the one shape the table cannot hold a row
//! for: `@membership.user.expects :reset_remote_connections`,
//! `Webhook.any_instance.stubs(:post).raises(Net::OpenTimeout)`. The
//! same closed-set argument applies, so the METHOD gets the slot. The
//! chain becomes a `MochaStub` (runtime/spinel/mocha_stub.rb) parked on
//! the instance (`user.__mocha_reset_remote_connections = …`, for an
//! object head whose static type names an app model) or filed by
//! "Klass#m" in the runtime's registry (for `any_instance`), and
//! `guard_app_methods` prepends to the lowered method a check that asks
//! for the stub first and, when one is in force, records the call,
//! raises the class the chains named (one arm per class, spelled as the
//! constant), and returns nil in place of the body — mocha's
//! replacement. Served: a bare `expects`, `.never`/`.once`/`.twice`/
//! `.times(n)`, `.raises(E)`; a `.returns(v)` or a `.with` would widen
//! the app method's return type or need argument matching, and goes to
//! the bridge (an instance head with no app-model type is left as
//! written).

use std::collections::{BTreeMap, BTreeSet};

use crate::app::App;
use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, ModelBodyItem, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

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
    /// `.with(a).returns(v1, v2, …)` — files a SEQUENCE under `a`:
    /// consecutive calls answer consecutive values, the last repeating.
    keyed_seq: Option<&'static str>,
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
    /// `expects(:m).with { |*a| … }[.never/.times(n)]` — files the count
    /// against the calls the block admits, as `(n, pred)`.
    expect_where: Option<&'static str>,
    /// `expects(:m).with { |*a| … }.throws(:tag)` — the calls the block
    /// admits `throw` the tag, as `("tag", pred)`: the tag travels as a
    /// String and the slot throws `tag.to_sym` (matz/spinel#4523).
    expect_throws_where: Option<&'static str>,
    /// `expects(:m)[.times(n)].raises(e)` — files the count and makes
    /// every call raise, as `(n, "<Class>")`: the exception travels by
    /// CLASS NAME, whether the test wrote the class or an instance of
    /// it (`raises(WebPush::ExpiredSubscription.new(…))`), because the
    /// slot re-raises one of its own hierarchy by name and the app
    /// rescues by class. An instance's constructor arguments are not
    /// carried; nothing rescues on a message.
    expect_raises: Option<&'static str>,
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
        keyed_seq: Some("stub_getaddresses_seq"),
        any: Some("stub_getaddresses_any"),
        bare: None,
        raises: Some("stub_getaddresses_raises"),
        where_: Some("stub_getaddresses_where"),
        expect: None,
        expect_entry: None,
        expect_where: None,
        expect_throws_where: None,
        expect_raises: None,
        clear: "clear_getaddresses_stubs",
        verify: None,
        require: "../runtime/resolv",
        slot_on: None,
    },
    // The socket the HTTP client opens. campfire's DNS-rebinding tests
    // put block-predicate expectations on it — `.never` for the
    // hostname, `.throws(:dns_not_rebound)` for the resolved address —
    // and the table (`runtime/spinel/tcp_socket_stub.rb`) is asked by
    // the client just before it connects, on both families.
    Stubbable {
        konst: "TCPSocket",
        method: "open",
        keyed: None,
        keyed_seq: None,
        any: None,
        bare: None,
        raises: None,
        where_: None,
        expect: None,
        expect_entry: None,
        expect_where: Some("expect_open_where"),
        expect_throws_where: Some("expect_open_throws_where"),
        expect_raises: None,
        clear: "clear_open_expectations",
        verify: Some("verify_open_expectations"),
        require: "../runtime/tcp_socket_stub",
        slot_on: Some("TcpSocketStub"),
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
        keyed_seq: None,
        any: Some("stub_alphanumeric"),
        bare: None,
        raises: None,
        where_: None,
        expect: None,
        expect_entry: None,
        expect_where: None,
        expect_throws_where: None,
        expect_raises: None,
        clear: "clear_secure_random_stubs",
        verify: None,
        require: "../runtime/secure_random_stub",
        slot_on: None,
    },
    Stubbable {
        konst: "SecureRandom",
        method: "uuid",
        keyed: None,
        keyed_seq: None,
        any: Some("stub_uuid"),
        bare: None,
        raises: None,
        where_: None,
        expect: None,
        expect_entry: None,
        expect_where: None,
        expect_throws_where: None,
        expect_raises: None,
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
        keyed_seq: None,
        any: Some("stub_uuid"),
        bare: None,
        raises: None,
        where_: None,
        expect: None,
        expect_entry: None,
        expect_where: None,
        expect_throws_where: None,
        expect_raises: None,
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
        keyed_seq: None,
        any: Some("stub_payload_send_any"),
        bare: Some("stub_payload_send"),
        raises: None,
        where_: None,
        expect: Some("expect_payload_send"),
        expect_entry: Some("expect_payload_send_with_entry"),
        expect_where: None,
        expect_throws_where: None,
        expect_raises: Some("expect_payload_send_raising"),
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
        keyed_seq: None,
        any: None,
        bare: None,
        raises: None,
        where_: None,
        expect: Some("expect_broadcast_replace_to"),
        expect_entry: None,
        expect_where: None,
        expect_throws_where: None,
        expect_raises: None,
        clear: "clear_broadcast_expectations",
        verify: Some("verify_broadcast_expectations"),
        require: "../runtime/turbo_streams",
        slot_on: None,
    },
    Stubbable {
        konst: "Turbo::StreamsChannel",
        method: "broadcast_remove_to",
        keyed: None,
        keyed_seq: None,
        any: None,
        bare: None,
        raises: None,
        where_: None,
        expect: Some("expect_broadcast_remove_to"),
        expect_entry: None,
        expect_where: None,
        expect_throws_where: None,
        expect_raises: None,
        clear: "clear_broadcast_expectations",
        verify: Some("verify_broadcast_expectations"),
        require: "../runtime/turbo_streams",
        slot_on: None,
    },
];

/// The file that defines `MochaBridge` — per target, see `project.rs`.
const BRIDGE_REQUIRE: &str = "../runtime/mocha_bridge";

/// The file that defines `MochaStub`, the app-method slot.
const APP_STUB_REQUIRE: &str = "../runtime/mocha_stub";

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
    for s in STUBBABLE.iter().map(|s| s.require).chain([BRIDGE_REQUIRE, APP_STUB_REQUIRE]) {
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
    out.push_str(&format!("{indent}MochaStub.clear!\n"));
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
    out.push_str(&format!("{indent}MochaStub.verify_all!\n"));
    out
}

pub fn apply_mocha_lowering(app: &mut App) {
    let mut ctx = AppMethods::index(app);
    for tm in &mut app.test_modules {
        ctx.setup = tm.setup.clone();
        if let Some(setup) = &mut tm.setup {
            rewrite(setup, &mut ctx);
        }
        for t in &mut tm.tests {
            rewrite(&mut t.body, &mut ctx);
        }
        for m in &mut tm.helpers {
            rewrite(&mut m.body, &mut ctx);
        }
    }
    guard_app_methods(app, &ctx.stubbed);
}

/// The app's instance methods, by model — what an instance or
/// `any_instance` chain may name — and the (model, method) pairs the
/// test bodies stubbed, with the exception classes their chains raise.
struct AppMethods {
    methods: BTreeMap<String, BTreeSet<String>>,
    stubbed: BTreeMap<(String, String), BTreeSet<String>>,
    /// Fixture accessor name -> the model class its rows are.
    fixtures: BTreeMap<String, String>,
    /// (model, singular association) -> target, for `belongs_to` and
    /// `has_one` reads.
    singular_assocs: BTreeMap<(String, String), String>,
    /// The enclosing test module's `setup` body, for an ivar's origin.
    setup: Option<Expr>,
}

impl AppMethods {
    fn index(app: &App) -> Self {
        let mut methods: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut singular_assocs: BTreeMap<(String, String), String> = BTreeMap::new();
        for m in &app.models {
            let class = m.name.0.as_str().to_string();
            let names = methods.entry(class.clone()).or_default();
            for def in m.methods() {
                if def.receiver == MethodReceiver::Instance && def.kind == AccessorKind::Method {
                    names.insert(def.name.as_str().to_string());
                }
            }
            for a in m.associations() {
                match a {
                    crate::dialect::Association::BelongsTo { name, target, polymorphic: false, .. }
                    | crate::dialect::Association::HasOne { name, target, .. } => {
                        singular_assocs.insert((class.clone(), name.as_str().to_string()), target.0.as_str().to_string());
                    }
                    _ => {}
                }
            }
        }
        let fixtures = app
            .fixtures
            .iter()
            .map(|f| (f.name.as_str().to_string(), crate::naming::classify_path(f.path.as_str())))
            .collect();
        AppMethods { methods, stubbed: BTreeMap::new(), fixtures, singular_assocs, setup: None }
    }

    #[cfg(test)]
    fn empty() -> Self {
        AppMethods {
            methods: BTreeMap::new(),
            stubbed: BTreeMap::new(),
            fixtures: BTreeMap::new(),
            singular_assocs: BTreeMap::new(),
            setup: None,
        }
    }

    fn has(&self, class: &str, method: &str) -> bool {
        self.methods.get(class).is_some_and(|ms| ms.contains(method))
    }

    /// The app class an object head is an instance of, read off the
    /// shapes a test body writes rather than off a type: this pass runs
    /// before the test modules are lowered and typed, so `@membership`
    /// carries no `ty` yet. A fixture call (`users(:david)`) is its
    /// model; an ivar is whatever the setup assigned it; a bare read
    /// off either (`@membership.user`) follows a `belongs_to`/`has_one`.
    /// Anything else is not a head this pass names.
    fn class_of_head(&self, recv: &Expr) -> Option<String> {
        if let Some(id) = recv.ty.as_ref().and_then(class_of) {
            return Some(id.0.as_str().to_string());
        }
        match &*recv.node {
            ExprNode::Send { recv: None, method, args, block: None, .. } if args.len() == 1 => {
                self.fixtures.get(method.as_str()).cloned()
            }
            ExprNode::Ivar { name } => {
                let setup = self.setup.as_ref()?;
                let assigned = find_ivar_assignment(setup, name)?;
                self.class_of_head(&assigned)
            }
            ExprNode::Send { recv: Some(base), method, args, block: None, .. } if args.is_empty() => {
                let owner = self.class_of_head(base)?;
                self.singular_assocs.get(&(owner, method.as_str().to_string())).cloned()
            }
            _ => None,
        }
    }
}

/// The value the last `@name = …` in `body` assigns, if any.
fn find_ivar_assignment(body: &Expr, name: &Symbol) -> Option<Expr> {
    let mut found: Option<Expr> = None;
    fn walk(e: &Expr, name: &Symbol, found: &mut Option<Expr>) {
        if let ExprNode::Assign { target: LValue::Ivar { name: n }, value } = &*e.node {
            if n == name {
                *found = Some(value.clone());
            }
        }
        e.node.for_each_child(&mut |c| walk(c, name, found));
    }
    walk(body, name, &mut found);
    found
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
    /// The constant as spelled, `::`-joined — or, for an instance head,
    /// the class its static type names.
    path: String,
    any_instance: bool,
    /// `obj.expects(:m)` — the object, whose static type is `path`.
    instance: Option<Expr>,
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
fn parse_chain(expr: &Expr, ctx: &AppMethods) -> Option<Chain> {
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
            let (konst, any_instance, instance) = match &*recv.node {
                ExprNode::Const { .. } => (recv.clone(), false, None),
                ExprNode::Send { recv: Some(k), method: ai, args: a, block: None, .. }
                    if ai.as_str() == "any_instance"
                        && a.is_empty()
                        && matches!(&*k.node, ExprNode::Const { .. }) =>
                {
                    (k.clone(), true, None)
                }
                // An object head: the class is whatever the analyzer
                // typed the receiver as (`@membership.user` — `User?`,
                // the nil arm stripped). Untyped, and this is not a
                // chain this pass reads.
                _ => {
                    let class = ctx.class_of_head(recv)?;
                    let konst = Expr::new(
                        recv.span,
                        ExprNode::Const { path: class.split("::").map(Symbol::from).collect() },
                    );
                    (konst, false, Some(recv.clone()))
                }
            };
            let ExprNode::Const { path } = &*konst.node else { return None };
            let path = path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::");
            ops.reverse();
            return Some(Chain { konst, path, any_instance, instance, kind: method.clone(), method: stubbed.clone(), ops });
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
fn rewrite(expr: &mut Expr, ctx: &mut AppMethods) {
    if let Some(chain) = parse_chain(expr, ctx) {
        let span = expr.span;
        if let Some(lowered) = lower_chain(span, chain, ctx) {
            *expr = lowered;
        }
        return;
    }
    expr.node.for_each_child_mut(&mut |c| rewrite(c, ctx));
}

/// `None` leaves the expression as written: an object head naming an
/// app class that has no such method is not this pass's to judge.
fn lower_chain(span: crate::span::Span, chain: Chain, ctx: &mut AppMethods) -> Option<Expr> {
    if chain.instance.is_none() && !chain.any_instance {
        if let Some(row) = row_for(&chain.path, chain.method.as_str()) {
            if let Some(served) = lower_known(span, &chain, row) {
                return Some(served);
            }
        }
    }
    if ctx.has(&chain.path, chain.method.as_str()) {
        if let Some(served) = lower_app_method(span, &chain, ctx) {
            return Some(served);
        }
    }
    if chain.instance.is_some() {
        return None;
    }
    Some(bridge_chain(span, chain))
}

/// The class a receiver's static type names, its nil arm stripped.
fn class_of(ty: &Ty) -> Option<ClassId> {
    match ty.clone().strip_nil() {
        Ty::Class { id, .. } => Some(id),
        _ => None,
    }
}

/// An app-method chain as its `MochaStub`:
///
/// ```text
///   obj.expects(:m)                 obj.__mocha_m = MochaStub.expect("K#m", 1)
///   obj.expects(:m).never           …expect("K#m", 0)
///   K.any_instance.stubs(:m)        MochaStub.file_any_instance("K#m", MochaStub.stub("K#m"))
///   K.any_instance.stubs(:m).raises(E)   …stub("K#m").raising("E")
/// ```
///
/// `None` for a link this slot does not serve — `returns`, `with`, a
/// second raise — and the chain goes on to the bridge (or, on an
/// object head, stays as written).
fn lower_app_method(span: crate::span::Span, chain: &Chain, ctx: &mut AppMethods) -> Option<Expr> {
    let plain = |op: &Op, name: &str, arity: usize| op.name.as_str() == name && op.args.len() == arity && op.block.is_none();
    let mut count: Option<Expr> = None;
    let mut raises: Option<String> = None;
    for op in &chain.ops {
        if let Some(n) = count_of(op) {
            if count.is_some() {
                return None;
            }
            count = Some(n);
        } else if plain(op, "raises", 1) {
            if raises.is_some() {
                return None;
            }
            raises = Some(exception_class_name(&op.args[0])?);
        } else {
            return None;
        }
    }
    let name = format!("{}#{}", chain.path, chain.method.as_str());
    let mocha_stub = Expr::new(span, ExprNode::Const { path: vec![Symbol::from("MochaStub")] });
    let mut stub = match chain.kind.as_str() {
        "expects" => call(span, mocha_stub.clone(), "expect", vec![str_lit(span, &name), count.unwrap_or_else(|| int_lit(span, 1))]),
        "stubs" if count.is_none() => call(span, mocha_stub.clone(), "stub", vec![str_lit(span, &name)]),
        _ => return None,
    };
    if let Some(e) = &raises {
        stub = call(span, stub, "raising", vec![str_lit(span, e)]);
    }
    let key = (chain.path.clone(), chain.method.as_str().to_string());
    let exceptions = ctx.stubbed.entry(key).or_default();
    if let Some(e) = raises {
        exceptions.insert(e);
    }
    Some(match &chain.instance {
        Some(obj) => Expr::new(
            span,
            ExprNode::Assign {
                target: LValue::Attr { recv: obj.clone(), name: Symbol::from(slot_name(chain.method.as_str())) },
                value: stub,
            },
        ),
        None => call(span, mocha_stub, "file_any_instance", vec![str_lit(span, &name), stub]),
    })
}

/// `__mocha_<m>` — the instance slot's name, and the ivar behind it.
fn slot_name(method: &str) -> String {
    format!("__mocha_{}", method.trim_end_matches(['?', '!']))
}

/// Give every stubbed app method its slot: the `__mocha_<m>=` writer, and
/// the guard at the top of the body.
///
/// ```ruby
///   def __mocha_post=(value); @__mocha_post = value; end
///   def post(payload)
///     __stub = @__mocha_post.nil? ? MochaStub.any_instance_for("Webhook#post") : @__mocha_post
///     if !__stub.nil?
///       __stub.record!
///       raise Net::OpenTimeout if __stub.raises == "Net::OpenTimeout"
///       return nil
///     end
///     <body>
///   end
/// ```
///
/// The raise arms are one per class the chains named for THIS method,
/// spelled as the constant — no name-to-class lookup at run time. The
/// `return nil` is mocha's replacement semantics; it widens the
/// method's return to nullable, which every lane carries for free.
fn guard_app_methods(app: &mut App, stubbed: &BTreeMap<(String, String), BTreeSet<String>>) {
    for ((class, method), exceptions) in stubbed {
        let Some(model) = app.models.iter_mut().find(|m| m.name.0.as_str() == class) else { continue };
        let slot = Symbol::from(slot_name(method));
        let ivar = slot.clone();
        let sp = crate::span::Span::synthetic;
        let Some(def) = model.methods_mut().find(|d| d.name.as_str() == method && d.receiver == MethodReceiver::Instance) else {
            continue;
        };
        // TYPED as it is built: this runs after analysis, and the
        // diagnostics walker reads an untyped ivar as `has no known
        // type` — an error on the strict emit, which is what the
        // archive build and the conformance ceiling run.
        let with_ty = crate::lower::typing::with_ty;
        let stub_ty = Ty::Union { variants: vec![Ty::Class { id: ClassId(Symbol::from("MochaStub")), args: vec![] }, Ty::Nil] };
        let stub_var = || with_ty(Expr::new(sp(), ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from("__stub") }), stub_ty.clone());
        let ivar_read = || with_ty(Expr::new(sp(), ExprNode::Ivar { name: ivar.clone() }), stub_ty.clone());
        let nil_q = |e: Expr| with_ty(call(sp(), e, "nil?", vec![]), Ty::Bool);
        let not = |e: Expr| with_ty(Expr::new(sp(), ExprNode::Send { recv: Some(e), method: Symbol::from("!"), args: vec![], block: None, parenthesized: false }), Ty::Bool);
        let mocha_stub = Expr::new(sp(), ExprNode::Const { path: vec![Symbol::from("MochaStub")] });
        let name = format!("{class}#{method}");
        let pick = with_ty(
            Expr::new(
                sp(),
                ExprNode::If {
                    cond: nil_q(ivar_read()),
                    then_branch: with_ty(call(sp(), mocha_stub, "any_instance_for", vec![str_lit(sp(), &name)]), stub_ty.clone()),
                    else_branch: ivar_read(),
                },
            ),
            stub_ty.clone(),
        );
        let bind = Expr::new(sp(), ExprNode::Assign { target: LValue::Var { id: crate::ident::VarId(0), name: Symbol::from("__stub") }, value: pick });
        let mut arm: Vec<Expr> = vec![with_ty(call(sp(), stub_var(), "record!", vec![]), Ty::Nil)];
        for e in exceptions {
            let konst = Expr::new(sp(), ExprNode::Const { path: e.split("::").map(Symbol::from).collect() });
            let raise = Expr::new(sp(), ExprNode::Raise { value: konst });
            let is = with_ty(
                Expr::new(
                    sp(),
                    ExprNode::Send { recv: Some(with_ty(call(sp(), stub_var(), "raises", vec![]), Ty::Str)), method: Symbol::from("=="), args: vec![str_lit(sp(), e)], block: None, parenthesized: false },
                ),
                Ty::Bool,
            );
            arm.push(Expr::new(sp(), ExprNode::If { cond: is, then_branch: raise, else_branch: Expr::new(sp(), ExprNode::Lit { value: Literal::Nil }) }));
        }
        arm.push(Expr::new(sp(), ExprNode::Return { value: Expr::new(sp(), ExprNode::Lit { value: Literal::Nil }) }));
        let guard = Expr::new(
            sp(),
            ExprNode::If {
                cond: not(nil_q(stub_var())),
                then_branch: Expr::new(sp(), ExprNode::Seq { exprs: arm }),
                else_branch: Expr::new(sp(), ExprNode::Lit { value: Literal::Nil }),
            },
        );
        let body = std::mem::replace(&mut def.body, Expr::new(sp(), ExprNode::Lit { value: Literal::Nil }));
        let mut exprs = vec![bind, guard];
        match *body.node {
            ExprNode::Seq { exprs: rest } => exprs.extend(rest),
            _ => exprs.push(body),
        }
        def.body = Expr::new(sp(), ExprNode::Seq { exprs });

        // The writer. Typed by its signature, since this runs after
        // analysis: the slot is `MochaStub?`.
        let value = Symbol::from("value");
        let writer = MethodDef {
            name_span: sp(),
            name: Symbol::from(format!("{}=", slot.as_str())),
            receiver: MethodReceiver::Instance,
            params: vec![Param::positional(value.clone())],
            body: Expr::new(
                sp(),
                ExprNode::Assign {
                    target: LValue::Ivar { name: ivar.clone() },
                    value: Expr::new(sp(), ExprNode::Var { id: crate::ident::VarId(0), name: value.clone() }),
                },
            ),
            signature: Some(crate::lower::typing::fn_sig(
                vec![(value, Ty::Union { variants: vec![Ty::Class { id: ClassId(Symbol::from("MochaStub")), args: vec![] }, Ty::Nil] })],
                Ty::Nil,
            )),
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: true,
            block_param: None,
        };
        model.body.push(ModelBodyItem::Method { method: writer, leading_comments: Vec::new(), leading_blank_line: true });
    }
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
            // `.with(a).returns(v1, v2, …)`: the values as ONE Array, in
            // order — the slot answers them consecutively.
            [with, ret] if plain(with, "with", 1) && ret.name.as_str() == "returns" && ret.args.len() > 1 && ret.block.is_none() => {
                row.keyed_seq.map(|seq| call(span, konst, seq, vec![with.args[0].clone(), array_lit(span, ret.args.clone())]))
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
            // A `.with(has_entry(k: v))` or `.raises(e)` link beside at
            // most one count.
            let entries: Vec<&Op> = ops.iter().filter(|op| op.name.as_str() == "with").collect();
            let raises: Vec<&Op> = ops.iter().filter(|op| op.name.as_str() == "raises").collect();
            let throws: Vec<&Op> = ops.iter().filter(|op| op.name.as_str() == "throws").collect();
            let counts: Vec<&Op> = ops
                .iter()
                .filter(|op| !matches!(op.name.as_str(), "with" | "raises" | "throws"))
                .collect();
            // `.with { |*a| … }.throws(:tag)`: no count — the throw leaves
            // the call before anything could be counted.
            if let ([with], [], [thr]) = (entries.as_slice(), raises.as_slice(), throws.as_slice()) {
                if with.name.as_str() == "with" && with.args.is_empty() && with.block.is_some() && plain(thr, "throws", 1) && counts.is_empty() {
                    let ExprNode::Lit { value: Literal::Sym { value: tag } } = &*thr.args[0].node else { return None };
                    let pred = predicate_lambda(with.block.clone()?)?;
                    return Some(call(span, konst, row.expect_throws_where?, vec![str_lit(span, tag.as_str()), pred]));
                }
            }
            if !throws.is_empty() {
                return None;
            }
            let n = match counts.as_slice() {
                // mocha reads a bare `expects` as exactly once.
                [] => int_lit(span, 1),
                [count] => count_of(count)?,
                _ => return None,
            };
            match (entries.as_slice(), raises.as_slice()) {
                ([], []) => Some(call(span, konst, row.expect?, vec![n])),
                ([with], []) if plain(with, "with", 1) => {
                    let (key, value) = has_entry_pair(&with.args[0])?;
                    Some(call(span, konst, row.expect_entry?, vec![n, sym_lit(span, key.as_str()), value]))
                }
                // `.with { |*a| … }[.never]`: the block is the predicate,
                // handed to the slot as a lambda over what the seam
                // passes.
                ([with], []) if with.name.as_str() == "with" && with.args.is_empty() && with.block.is_some() => {
                    let pred = predicate_lambda(with.block.clone()?)?;
                    Some(call(span, konst, row.expect_where?, vec![n, pred]))
                }
                ([], [raises]) if plain(raises, "raises", 1) => {
                    let name = exception_class_name(&raises.args[0])?;
                    Some(call(span, konst, row.expect_raises?, vec![n, str_lit(span, &name)]))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The class a `raises(e)` link names, whether `e` is the class or a
/// `<Class>.new(…)` of it. Anything else has no name to carry.
fn exception_class_name(arg: &Expr) -> Option<String> {
    let path = match &*arg.node {
        ExprNode::Const { path } => path,
        ExprNode::Send { recv: Some(r), method, .. } if method.as_str() == "new" => match &*r.node {
            ExprNode::Const { path } => path,
            _ => return None,
        },
        _ => return None,
    };
    Some(path.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("::"))
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
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "Resolv");
        assert_eq!(m, "stub_getaddresses");
        assert_eq!(args.len(), 2);
    }

    #[test]
    fn a_keyed_returns_with_several_values_files_a_sequence() {
        // Resolv.stubs(:getaddresses).with("h").returns([a], [b])
        let mut e = send(
            Some(send(Some(send(Some(konst(&["Resolv"])), "stubs", vec![sym("getaddresses")])), "with", vec![str_lit(sp(), "h")])),
            "returns",
            vec![array_lit(sp(), vec![str_lit(sp(), "1.2.3.4")]), array_lit(sp(), vec![str_lit(sp(), "127.0.0.1")])],
        );
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "Resolv");
        assert_eq!(m, "stub_getaddresses_seq");
        assert_eq!(args.len(), 2, "the host, then the answers as one Array");
        assert!(matches!(&*args[1].node, ExprNode::Array { elements, .. } if elements.len() == 2));
    }

    #[test]
    fn a_socket_expectation_with_a_block_and_never_files_a_count_on_the_seam() {
        // TCPSocket.expects(:open).with { |*args| … }.never
        let head = send(Some(konst(&["TCPSocket"])), "expects", vec![sym("open")]);
        let with = send_blk(head, "with", lambda());
        let mut e = send(Some(with), "never", vec![]);
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, block) = as_send(&e);
        assert_eq!(const_path(recv), "TcpSocketStub", "the slot lives on the seam, not the stdlib class");
        assert_eq!(m, "expect_open_where");
        assert_eq!(args.len(), 2);
        assert_eq!(int_of(&args[0]), 0);
        assert!(matches!(&*args[1].node, ExprNode::Lambda { .. }));
        assert!(block.is_none());
    }

    #[test]
    fn a_socket_expectation_that_throws_carries_the_tag_by_name() {
        // TCPSocket.expects(:open).with { |*args| … }.throws(:dns_not_rebound)
        let head = send(Some(konst(&["TCPSocket"])), "expects", vec![sym("open")]);
        let with = send_blk(head, "with", lambda());
        let mut e = send(Some(with), "throws", vec![sym("dns_not_rebound")]);
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "TcpSocketStub");
        assert_eq!(m, "expect_open_throws_where");
        assert_eq!(args.len(), 2);
        assert!(
            matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == "dns_not_rebound"),
            "the tag travels as a String (matz/spinel#4523): {:?}",
            args[0].node
        );
        assert!(matches!(&*args[1].node, ExprNode::Lambda { .. }));
    }

    #[test]
    fn a_bare_stubs_lowers_to_the_rows_default() {
        let mut e = send(Some(konst(&["WebPush"])), "stubs", vec![sym("payload_send")]);
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "WebPush");
        assert_eq!(m, "stub_payload_send");
        assert!(args.is_empty());
    }

    #[test]
    fn expects_times_raises_files_the_count_and_the_class_name() {
        // WebPush.expects(:payload_send).times(2).raises(WebPush::ExpiredSubscription.new(x, "example.com"))
        let head = send(Some(konst(&["WebPush"])), "expects", vec![sym("payload_send")]);
        let times = send(Some(head), "times", vec![Expr::new(sp(), ExprNode::Lit { value: Literal::Int { value: 2 } })]);
        let instance = send(Some(konst(&["WebPush", "ExpiredSubscription"])), "new", vec![str_lit(sp(), "example.com")]);
        let mut e = send(Some(times), "raises", vec![instance]);
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "WebPush");
        assert_eq!(m, "expect_payload_send_raising");
        assert_eq!(int_of(&args[0]), 2);
        assert!(matches!(&*args[1].node, ExprNode::Lit { value: Literal::Str { value } } if value == "WebPush::ExpiredSubscription"));

        // The class itself names the same slot; a row without the slot
        // (Resolv's `expects` has none) goes to the bridge.
        let head = send(Some(konst(&["WebPush"])), "expects", vec![sym("payload_send")]);
        let mut e = send(Some(head), "raises", vec![konst(&["WebPush", "Unauthorized"])]);
        rewrite(&mut e, &mut AppMethods::empty());
        let (_, m, args, _) = as_send(&e);
        assert_eq!(m, "expect_payload_send_raising");
        assert_eq!(int_of(&args[0]), 1);
        assert!(matches!(&*args[1].node, ExprNode::Lit { value: Literal::Str { value } } if value == "WebPush::Unauthorized"));
    }

    #[test]
    fn expects_with_a_count_files_the_count() {
        for (link, n) in [("never", 0), ("once", 1), ("twice", 2)] {
            let mut e = send(Some(send(Some(konst(&["WebPush"])), "expects", vec![sym("payload_send")])), link, vec![]);
            rewrite(&mut e, &mut AppMethods::empty());
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
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "Turbo::StreamsChannel");
        assert_eq!(m, "expect_broadcast_remove_to");
        assert_eq!(int_of(&args[0]), 3);
        let mut e = send(Some(konst(&["Turbo", "StreamsChannel"])), "expects", vec![sym("broadcast_replace_to")]);
        rewrite(&mut e, &mut AppMethods::empty());
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
        rewrite(&mut e, &mut AppMethods::empty());
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
        rewrite(&mut e, &mut AppMethods::empty());
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
        rewrite(&mut e, &mut AppMethods::empty());
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "Resolv");
        assert_eq!(m, "stub_getaddresses_raises");
        assert_eq!(args.len(), 1);

        // WebPush.expects(:payload_send).with(has_entry(endpoint_ip: ip)) — bare expects is once.
        let matcher = send(None, "has_entry", vec![Expr::new(sp(), ExprNode::Hash { entries: vec![(sym("endpoint_ip"), str_lit(sp(), "1.2.3.4"))], kwargs: true })]);
        let head = send(Some(konst(&["WebPush"])), "expects", vec![sym("payload_send")]);
        let mut e = send(Some(head), "with", vec![matcher]);
        rewrite(&mut e, &mut AppMethods::empty());
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
        rewrite(&mut e, &mut AppMethods::empty());
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

    /// An app with one model, `User`, carrying `reset_remote_connections`,
    /// a `users` fixture, and a `Membership` whose `user` is a
    /// `belongs_to`.
    fn app_ctx() -> AppMethods {
        let mut ctx = AppMethods::empty();
        ctx.methods.entry("User".into()).or_default().insert("reset_remote_connections".into());
        ctx.methods.entry("Webhook".into()).or_default().insert("post".into());
        ctx.fixtures.insert("users".into(), "User".into());
        ctx.fixtures.insert("memberships".into(), "Membership".into());
        ctx.singular_assocs.insert(("Membership".into(), "user".into()), "User".into());
        ctx
    }

    #[test]
    fn an_expects_on_an_app_instance_parks_a_stub_on_the_object() {
        // @membership.user.expects :reset_remote_connections — with
        // `@membership = memberships(:one)` in setup.
        let mut ctx = app_ctx();
        ctx.setup = Some(Expr::new(
            sp(),
            ExprNode::Assign {
                target: crate::expr::LValue::Ivar { name: Symbol::from("membership") },
                value: send(None, "memberships", vec![sym("one")]),
            },
        ));
        let obj = send(Some(Expr::new(sp(), ExprNode::Ivar { name: Symbol::from("membership") })), "user", vec![]);
        let mut e = send(Some(obj), "expects", vec![sym("reset_remote_connections")]);
        rewrite(&mut e, &mut ctx);
        let ExprNode::Assign { target: crate::expr::LValue::Attr { name, .. }, value } = &*e.node else { panic!("{:?}", e.node) };
        assert_eq!(name.as_str(), "__mocha_reset_remote_connections");
        let (recv, m, args, _) = as_send(value);
        assert_eq!(const_path(recv), "MochaStub");
        assert_eq!(m, "expect");
        assert!(matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == "User#reset_remote_connections"));
        assert_eq!(int_of(&args[1]), 1, "a bare expects is once");
        assert!(ctx.stubbed.contains_key(&("User".to_string(), "reset_remote_connections".to_string())));
    }

    #[test]
    fn an_any_instance_stub_that_raises_files_by_name_and_records_the_class() {
        // Webhook.any_instance.stubs(:post).raises(Net::OpenTimeout)
        let mut ctx = app_ctx();
        let head = send(Some(send(Some(konst(&["Webhook"])), "any_instance", vec![])), "stubs", vec![sym("post")]);
        let mut e = send(Some(head), "raises", vec![konst(&["Net", "OpenTimeout"])]);
        rewrite(&mut e, &mut ctx);
        let (recv, m, args, _) = as_send(&e);
        assert_eq!(const_path(recv), "MochaStub");
        assert_eq!(m, "file_any_instance");
        assert!(matches!(&*args[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == "Webhook#post"));
        let (stub, raising, rargs, _) = as_send(&args[1]);
        assert_eq!(raising, "raising");
        assert!(matches!(&*rargs[0].node, ExprNode::Lit { value: Literal::Str { value } } if value == "Net::OpenTimeout"));
        let (_, kind, _, _) = as_send(stub);
        assert_eq!(kind, "stub");
        let exceptions = &ctx.stubbed[&("Webhook".to_string(), "post".to_string())];
        assert!(exceptions.contains("Net::OpenTimeout"), "the guard gets one raise arm per class");
    }

    #[test]
    fn an_app_method_chain_with_a_returns_goes_on_to_the_bridge() {
        // Webhook.any_instance.stubs(:post).returns(x) — a value would
        // widen the app method's return; not served here.
        let mut ctx = app_ctx();
        let head = send(Some(send(Some(konst(&["Webhook"])), "any_instance", vec![])), "stubs", vec![sym("post")]);
        let mut e = send(Some(head), "returns", vec![sym("x")]);
        rewrite(&mut e, &mut ctx);
        let (recv, m, _, _) = as_send(&e);
        assert_eq!(const_path(recv), "MochaBridge");
        assert_eq!(m, "chain");
        assert!(ctx.stubbed.is_empty());
    }

    #[test]
    fn an_object_head_without_an_app_class_is_left_as_written() {
        // something.expects(:m) where nothing says what `something` is.
        let mut ctx = app_ctx();
        let obj = send(None, "something", vec![]);
        let mut e = send(Some(obj), "expects", vec![sym("m")]);
        rewrite(&mut e, &mut ctx);
        let (_, m, _, _) = as_send(&e);
        assert_eq!(m, "expects");
    }

    #[test]
    fn a_send_that_is_not_a_chain_is_left_alone() {
        let mut e = send(Some(konst(&["Resolv"])), "getaddresses", vec![str_lit(sp(), "h")]);
        let before = format!("{:?}", e);
        rewrite(&mut e, &mut AppMethods::empty());
        assert_eq!(format!("{:?}", e), before);
    }
}
