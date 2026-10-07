//! Recover a `**hash` argument the ingest desugar erased, expanding it
//! into the keyword arguments the callee actually declares.
//!
//! `ingest_hash_literal` rewrites every double splat into the `merge`
//! chain it is defined to be, which keeps `**` out of the IR entirely
//! (no new `ExprNode` variant, no match arm in thirteen emitters). Its
//! correctness argument is stated there:
//!
//! > A merge chain is a `Send`, not a `Hash`, so it loses the `kwargs`
//! > flag and renders as a positional hash argument. That matches the
//! > receiving end: `ingest_method_def` already models a `**rest`
//! > parameter as a trailing *positional* param.
//!
//! That holds when the callee declares `**rest` — both sides then agree
//! a keyword bundle is "one positional Hash". It does NOT hold when the
//! callee declares explicit keywords, because Ruby needs the `**` at the
//! call site to distribute the hash across them. campfire's
//! `Sound#initialize` is the shape:
//!
//! ```ruby
//! class Image < Struct.new(:asset_path, :width, :height)
//!   def initialize(name:, width:, height:)
//! # …
//! @image = Image.new(**image)     # → Image.new(image)
//! ```
//!
//! which raises `wrong number of arguments (given 1, expected 0;
//! required keywords: name, width, height)` — and, because the call sits
//! under a class-body constant, at `require` time rather than on a
//! request. Nothing static catches it: the call is well-typed, and the
//! arity mismatch only exists in the target language.
//!
//! ## Recovering the splat without an IR marker
//!
//! By the time a lowering runs, `f(**h)` and `f(h)` are the same tree.
//! They are still distinguishable, because only one of them can be
//! valid Ruby: since 3.0 a bare Hash is never auto-converted to
//! keywords, so a call passing one more positional argument than the
//! callee has positional parameters, to a callee with required keywords,
//! could ONLY have been written with `**`. Anything else already raises
//! under Rails, and the input is a working app.
//!
//! ## Expanding rather than re-splatting
//!
//! The rewrite reads the keyword names off the callee and indexes the
//! hash for each one:
//!
//! ```ruby
//! Image.new(name: image[:name], width: image[:width], height: image[:height])
//! ```
//!
//! A `Hash { kwargs: true }` trailing argument is the ordinary
//! keyword-call shape every target already emits, so this needs no new
//! IR — the same trade the merge-chain desugar made. Re-splatting
//! instead (marking the argument so ruby-family emitters render `**h`)
//! would preserve Ruby's semantics exactly, but it is unrepresentable on
//! the strict targets, which would then need this expansion anyway.
//! Expanding also types each argument individually instead of passing
//! one opaque Hash.
//!
//! ## Optional keywords
//!
//! An optional keyword — `filename: "Title"` — is read with the default
//! in hand: `filename: h.fetch(:filename, "Title")`, which is what Ruby's
//! `**` does for a key the bundle does not carry. That re-evaluates the
//! default in the CALLER's scope, so only a literal qualifies (a Symbol,
//! String, number, `nil`, an empty collection — the same rule
//! `kwrest_forward` applies, for the same reason: `Current.user` means
//! something else over here). campfire's test helper
//! `attachment_for(href:, url:, filename: "Title", caption:
//! "Description")`, reached through `embed_from(**attributes)`, is the
//! shape; all eight opengraph-embed tests tripped on it.
//!
//! ## Receiverless calls in a test class
//!
//! `embed_from(**attributes)` is a self-send: no receiver, so no
//! receiver type to look the callee up by. Inside a test class the
//! callee is the class's own helper, resolved per class the way
//! `helper_kwargs` resolves its test-module half — by name, declining a
//! name two helpers share. Test helpers KEEP their keyword parameters
//! through ingest (unlike library classes, which flatten them), so the
//! excess-argument evidence reads the same.
//!
//! ## The three guards, and why each one fails closed
//!
//! - **Every optional keyword's default must be a literal.** Anything
//!   else would be evaluated where the caller stands, not where the
//!   callee wrote it.
//! - **The hash expression must be a pure, cheap read** (local, ivar or
//!   constant). It is evaluated once per keyword, so a call or an
//!   arithmetic chain would be re-run N times.
//! - **The trailing argument must not already be `Hash { kwargs: true }`**
//!   — that is a literal keyword list, which renders correctly as-is and
//!   is `normalize_trailing_kwargs`' business, not this pass's.
//!
//! A site that trips a guard is left alone and LEDGERED, because leaving
//! it alone means it still dies at run time. That is the whole point of
//! the pass: turn an invisible load-time arity crash into either a fixed
//! call or a diagnostic.
//!
//! ## Known divergence
//!
//! `h[:k]` yields `nil` for a missing key where Ruby's `**` raises
//! `ArgumentError`, and an extra key in `h` is ignored where Ruby
//! raises. Both are malformed-input paths — the well-formed call is
//! exact. `fetch` would be closer for the missing-key half, but only
//! elixir emits the one-argument form; `[]` every target handles.

use std::collections::HashMap;

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{ControllerBodyItem, MethodReceiver, ModelBodyItem};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::dialect::Param;
use crate::ty::Ty;

/// Declared parameter lists, keyed by `(class, method)`.
///
/// Read off the App IR rather than the analyzer's class registry, which
/// is the authority for *inferred* types but not for declared shape:
/// `Sound::Image#initialize` resolves there as a bare `Ty::Untyped`,
/// carrying no parameter list at all, so a registry-driven lookup sees
/// nothing to compare the call against. What this pass needs — the
/// names and kinds the callee wrote down — is a syntactic fact of the
/// source, and the IR has it exactly.
#[derive(Default)]
struct Signatures {
    methods: HashMap<(ClassId, Symbol), Vec<Param>>,
    /// Controller `ClassMethod` (and copied Concern macros) parameter
    /// lists. Kept separate from `methods` so a receiverless call in a
    /// `def self.` cannot resolve to an instance method of the same name.
    class_methods: HashMap<(ClassId, Symbol), Vec<Param>>,
    /// Superclass links, so a call landing on an inherited `initialize`
    /// still resolves.
    parents: HashMap<ClassId, ClassId>,
}

pub fn apply_kwsplat_expansion(app: &mut App) -> Vec<Diagnostic> {
    // This public pass is also used outside the complete hook pipeline.
    // Project ordinary producers through the same source contract first.
    let mut diags = super::forwarding::apply(app);
    let sigs = collect_signatures(app);
    super::for_each_hook_body(app, &mut |body| rewrite(body, &sigs, &mut diags));
    apply_to_self_sends(app, &sigs, &mut diags);
    apply_to_test_modules(app, &mut diags);
    diags
}

/// Re-apply the `**h` → `KeywordSplat` restore after a later pass has
/// projected splats to positional hashes. `helper_kwargs` calls
/// `forwarding::apply`, which strips `**` so explicit-keyword expansion
/// can fire; a callee that IS `**rest` then needs the marker back.
pub(crate) fn restore_kwrest_in_test_helpers(app: &mut App) {
    let sigs = collect_signatures(app);
    apply_to_self_sends(app, &sigs, &mut Vec::new());
    apply_to_test_modules(app, &mut Vec::new());
}

/// The receiverless half for models, library classes, and controller
/// class methods: `render_code(size: 2, **opts)` inside the class that
/// defines `render_code`, or a Concern macro's
/// `add_preload_definition(kind: …, **options)` after
/// `ingest::class_attribute` copies it onto the includer. The body
/// typer leaves these sends `recv: None`, so the receiver-typed walk
/// above never sees them. Each class's view is its own methods, its
/// ancestors', and every module it includes, transitively.
///
/// Instance method bodies use the instance-method table; controller
/// `ClassMethod` bodies use the class-method table — a receiverless
/// call in a `def self.` reaches the class side.
fn apply_to_self_sends(app: &mut App, sigs: &Signatures, diags: &mut Vec<Diagnostic>) {
    let mut includes: HashMap<ClassId, Vec<ClassId>> = HashMap::new();
    for lc in &app.library_classes {
        includes.insert(lc.name.clone(), lc.includes.clone());
    }
    for model in &app.models {
        includes.insert(model.name.clone(), crate::analyze::model_includes(model));
    }
    for controller in &app.controllers {
        includes.insert(
            controller.name.clone(),
            crate::analyze::controller_includes(controller),
        );
    }
    let view_of = |id: &ClassId, class_side: bool| -> HashMap<Symbol, Vec<Param>> {
        let table = if class_side {
            &sigs.class_methods
        } else {
            &sigs.methods
        };
        let mut out: HashMap<Symbol, Vec<Param>> = HashMap::new();
        let mut queue: Vec<ClassId> = vec![id.clone()];
        let mut seen: std::collections::BTreeSet<ClassId> = std::collections::BTreeSet::new();
        while let Some(cid) = queue.pop() {
            if !seen.insert(cid.clone()) {
                continue;
            }
            for ((owner, name), params) in table {
                if *owner == cid {
                    // Nearest definition wins: the class itself is
                    // visited first, then what it reaches.
                    out.entry(name.clone()).or_insert_with(|| params.clone());
                }
            }
            if let Some(parent) = sigs.parents.get(&cid) {
                queue.push(parent.clone());
            }
            if let Some(incs) = includes.get(&cid) {
                queue.extend(incs.iter().cloned());
            }
        }
        out
    };
    for lc in &mut app.library_classes {
        let view = view_of(&lc.name, false);
        if view.is_empty() {
            continue;
        }
        for m in &mut lc.methods {
            if matches!(m.receiver, MethodReceiver::Instance) {
                rewrite_self_sends(&mut m.body, &view, diags);
            }
        }
    }
    for model in &mut app.models {
        let view = view_of(&model.name, false);
        if view.is_empty() {
            continue;
        }
        for item in &mut model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                if matches!(method.receiver, MethodReceiver::Instance) {
                    rewrite_self_sends(&mut method.body, &view, diags);
                }
            }
        }
    }
    // Concern macros writing a `class_attribute` are class methods on
    // the includer; their erased `**options` must expand here or the
    // emitted Ruby raises at class load (wrong number of arguments).
    for controller in &mut app.controllers {
        let view = view_of(&controller.name, true);
        if view.is_empty() {
            continue;
        }
        for item in &mut controller.body {
            if let ControllerBodyItem::ClassMethod { method, .. } = item {
                rewrite_self_sends(&mut method.body, &view, diags);
            }
        }
    }
}

/// The test-class half: a receiverless call in a test body, `setup` or
/// helper, resolved against the class's OWN helpers by name. A name two
/// helpers share is skipped — the call site does not say which.
fn apply_to_test_modules(app: &mut App, diags: &mut Vec<Diagnostic>) {
    for tm in &mut app.test_modules {
        let mut helpers: HashMap<Symbol, Vec<Param>> = HashMap::new();
        let mut ambiguous: Vec<Symbol> = Vec::new();
        for m in &tm.helpers {
            if helpers.insert(m.name.clone(), m.params.clone()).is_some() {
                ambiguous.push(m.name.clone());
            }
        }
        for name in ambiguous {
            helpers.remove(&name);
        }
        if helpers.is_empty() {
            continue;
        }
        let mut visit = |body: &mut Expr| rewrite_self_sends(body, &helpers, diags);
        if let Some(setup) = &mut tm.setup {
            visit(setup);
        }
        for t in &mut tm.tests {
            visit(&mut t.body);
        }
        for h in &mut tm.helpers {
            visit(&mut h.body);
        }
    }
}

fn rewrite_self_sends(expr: &mut Expr, helpers: &HashMap<Symbol, Vec<Param>>, diags: &mut Vec<Diagnostic>) {
    expr.node
        .for_each_child_mut(&mut |child| rewrite_self_sends(child, helpers, diags));
    let splat = {
        let ExprNode::Send { recv, method, args, .. } = &*expr.node else {
            return;
        };
        if !is_self_send(recv.as_ref()) {
            return;
        }
        let Some(params) = helpers.get(method) else { return };
        erased_splat_against(args, params)
    };
    let ExprNode::Send { method, args, .. } = &mut *expr.node else {
        unreachable!("matched a Send above")
    };
    if let Some(splat) = splat {
        expand(args, splat, diags);
        return;
    }
    let Some(params) = helpers.get(method) else { return };
    restore_kwrest_splat(args, params);
}

/// Every instance method an app class declares, plus every controller
/// class method. Instance and class-side tables stay separate: a
/// `Class.new(…)` call resolves to `initialize` on the instance table,
/// while a receiverless call in a `def self.` uses the class-side table.
fn collect_signatures(app: &App) -> Signatures {
    let mut sigs = Signatures::default();
    for lc in &app.library_classes {
        if let Some(parent) = &lc.parent {
            sigs.parents.insert(lc.name.clone(), parent.clone());
        }
        for m in &lc.methods {
            if matches!(m.receiver, MethodReceiver::Instance) {
                sigs.methods
                    .insert((lc.name.clone(), m.name.clone()), m.params.clone());
            }
        }
    }
    for model in &app.models {
        for item in &model.body {
            if let ModelBodyItem::Method { method, .. } = item {
                if matches!(method.receiver, MethodReceiver::Instance) {
                    sigs.methods.insert(
                        (model.name.clone(), method.name.clone()),
                        method.params.clone(),
                    );
                }
            }
        }
    }
    for controller in &app.controllers {
        if let Some(parent) = &controller.parent {
            sigs.parents.insert(controller.name.clone(), parent.clone());
        }
        for item in &controller.body {
            if let ControllerBodyItem::ClassMethod { method, .. } = item {
                sigs.class_methods.insert(
                    (controller.name.clone(), method.name.clone()),
                    method.params.clone(),
                );
            }
        }
    }
    sigs
}

/// The callee's keyword parameters, when this call is an erased `**`
/// splat into explicit keywords.
struct ErasedSplat {
    /// Keyword parameters in declaration order, each with the default
    /// it declares (None for a required keyword).
    keywords: Vec<(Symbol, Option<Expr>)>,
    /// False when some optional keyword's default is not a literal —
    /// the pass may not expand it, but still ledgers it.
    defaults_literal: bool,
}

fn rewrite(expr: &mut Expr, sigs: &Signatures, diags: &mut Vec<Diagnostic>) {
    expr.node
        .for_each_child_mut(&mut |child| rewrite(child, sigs, diags));

    if let Some(splat) = erased_splat(expr, sigs) {
        let ExprNode::Send { args, .. } = &mut *expr.node else {
            unreachable!("erased_splat matched a Send")
        };
        expand(args, splat, diags);
        return;
    }
    restore_kwrest_on_typed_send(expr, sigs);
}

fn is_self_send(recv: Option<&Expr>) -> bool {
    match recv {
        None => true,
        Some(r) => matches!(&*r.node, ExprNode::SelfRef),
    }
}

fn restore_kwrest_on_typed_send(expr: &mut Expr, sigs: &Signatures) {
    let ExprNode::Send { recv: Some(recv), method, .. } = &*expr.node else {
        return;
    };
    let Some(params) = recv.ty.as_ref().and_then(|ty| callee_params(ty, method, sigs)) else {
        return;
    };
    let params = params.clone();
    let ExprNode::Send { args, .. } = &mut *expr.node else { return };
    restore_kwrest_splat(args, &params);
}

/// `f(**h)` into `def f(**rest)` survived ingest as a positional `h`.
/// Ruby 3 will not auto-convert that Hash, so the call is
/// `wrong number of arguments (given 1, expected 0)` — campfire's
/// `embeds_from(**details)` → `attachments_for(details)` against
/// `def attachments_for(**details)`. Restore the splat; the ruby
/// emitter already prints `**h` for `KeywordSplat`.
fn restore_kwrest_splat(args: &mut Vec<Expr>, params: &[Param]) {
    if args
        .iter()
        .any(|a| matches!(&*a.node, ExprNode::ForwardArgs | ExprNode::KeywordSplat { .. }))
        || params.iter().any(|p| p.forwarding)
    {
        return;
    }
    // `def f(*items, **opts); f(payload)` is a valid positional call:
    // `*items` absorbs the Hash. The same count as `f(**payload)` into
    // a bare `**opts`, so a rewrite here would move the argument from
    // `items` onto `opts`. Leave the call when that distinction is
    // unavailable.
    if params.iter().any(|p| p.rest && !p.keyword && !p.from_kwrest) {
        return;
    }
    let positional = params
        .iter()
        .filter(|p| !p.keyword && !p.rest && !p.from_kwrest)
        .count();
    let Some(last) = params.last() else { return };
    // Ingest keeps `**rest` as a keyword-rest when the def already has
    // a rest/required-keyword or the body forwards `**name`. Otherwise
    // it flattens to a trailing positional marked `from_kwrest`, and
    // emit prints `name = {}`. Restoring `**h` against that def is
    // unexpected keywords. Only the kept `**name` slot needs the splat.
    if !(last.keyword && last.rest) {
        return;
    }
    if args.len() != positional + 1 {
        return;
    }
    let Some(hash) = args.last() else { return };
    if !is_pure_read(hash) {
        return;
    }
    let hash = args.pop().expect("checked above");
    args.push(Expr::new(
        hash.span,
        ExprNode::KeywordSplat { value: hash },
    ));
}

/// Replace the trailing positional bundle in `args` with the keyword
/// list `splat` names, or ledger why not.
fn expand(args: &mut Vec<Expr>, splat: ErasedSplat, diags: &mut Vec<Diagnostic>) {
    let hash = args.last().expect("erased_splat matched a trailing arg");

    if !splat.defaults_literal {
        diags.push(residue(
            hash,
            "callee declares an optional keyword whose default is not a literal",
        ));
        return;
    }
    let Some((read, literal, bundle_wins)) = splat_shape(hash) else {
        diags.push(residue(
            hash,
            "the splatted expression is not a local, ivar or constant read",
        ));
        return;
    };
    let (read, literal): (Expr, Vec<(Expr, Expr)>) = (read.clone(), literal.to_vec());
    // `f(k: compute(), **h)` evaluates `compute()` even when `h` has
    // `:k`. `h.fetch(:k, compute())` does not. Leave that call as the
    // positional merge; a literal value is the only default fetch can
    // hold without changing evaluation.
    if bundle_wins && literal.iter().any(|(_, v)| !is_literal(v) && !is_pure_read(v)) {
        diags.push(residue(
            hash,
            "a keyword value beside a later splat is not a literal, so fetch would skip it",
        ));
        return;
    }

    let hash = args.pop().expect("checked above");
    let value_ty = match &read.ty {
        Some(Ty::Hash { value, .. }) => (**value).clone(),
        _ => Ty::Untyped,
    };
    // A keyword the literal names takes the literal's value — evaluated
    // once, as Ruby's `**` would; the rest are read off the bundle, an
    // optional one with its declared default in hand. When the bundle
    // was splatted AFTER the literal (`f(size: 2, **opts)`), Ruby lets
    // the bundle's key win, so the literal is the default the bundle is
    // read against.
    let entries = splat
        .keywords
        .iter()
        .map(|(kw, default)| {
            let value = literal
                .iter()
                .find(|(k, _)| sym_of(k).is_some_and(|k| k == kw))
                .map(|(_, v)| {
                    if bundle_wins {
                        fetch(&read, kw, v, value_ty.clone())
                    } else {
                        v.clone()
                    }
                })
                .unwrap_or_else(|| match default {
                    Some(default) => fetch(&read, kw, default, value_ty.clone()),
                    None => index(&read, kw, value_ty.clone()),
                });
            (sym_key(kw, hash.span), value)
        })
        .collect();
    let mut kwargs = Expr::new(hash.span, ExprNode::Hash { entries, kwargs: true });
    kwargs.ty = hash.ty.clone();
    args.push(kwargs);
}

/// The two halves of a splatted expression this pass can expand: the
/// pure read of the bundle, and the literal pairs merged over it.
///
/// `f(**h)` is the bare read with no pairs. `f(**h, k: v)` — campfire's
/// `WebPush::Notification.new(**params, badge: …, endpoint: …)` — is the
/// merge chain the ingest desugar made of it, `h.merge({ k: v })`, and
/// is just as expandable: every key of the literal is a Symbol the
/// caller wrote, so the keyword it names takes that value and only the
/// keywords it does NOT name are indexed off `h`. `f(k: v, **h)` is
/// the mirror, `{ k: v }.merge(h)`, where the bundle's key wins over
/// the literal's; the third value says which order it was. A key that
/// is not a Symbol literal, or a merge whose other side is not a
/// literal, is a bundle this pass cannot read, and declines.
fn splat_shape(expr: &Expr) -> Option<(&Expr, &[(Expr, Expr)], bool)> {
    if is_pure_read(expr) {
        return Some((expr, &[], false));
    }
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "merge" || args.len() != 1 {
        return None;
    }
    let (read, literal, bundle_wins) = if is_pure_read(recv) {
        (recv, &args[0], false)
    } else if is_pure_read(&args[0]) {
        (&args[0], recv, true)
    } else {
        return None;
    };
    let ExprNode::Hash { entries, .. } = &*literal.node else { return None };
    if !entries.iter().all(|(k, _)| sym_of(k).is_some()) {
        return None;
    }
    Some((read, entries.as_slice(), bundle_wins))
}

fn sym_of(key: &Expr) -> Option<&Symbol> {
    match &*key.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value),
        _ => None,
    }
}

/// Is `expr` a call whose trailing positional argument can only have
/// been written `**hash`? See the header for why the excess-argument
/// count is sufficient evidence.
fn erased_splat(expr: &Expr, sigs: &Signatures) -> Option<ErasedSplat> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node else {
        return None;
    };
    let params = callee_params(recv.ty.as_ref()?, method, sigs)?;
    erased_splat_against(args, params)
}

/// The same question with the callee's parameter list already in hand.
fn erased_splat_against(args: &[Expr], params: &[Param]) -> Option<ErasedSplat> {
    let last = args.last()?;
    // Full packets are explicit source facts, not erased hash expressions
    // from which this pass can recover an anonymous keyword splat.
    if args.iter().any(|a| matches!(&*a.node, ExprNode::ForwardArgs | ExprNode::KeywordSplat { .. }))
        || params.iter().any(|p| p.forwarding)
    {
        return None;
    }
    // A literal keyword list already renders as keywords.
    if matches!(&*last.node, ExprNode::Hash { kwargs: true, .. }) {
        return None;
    }

    // A `*rest` parameter absorbs any number of positional arguments, so
    // the excess-argument evidence says nothing about this call.
    if params.iter().any(|p| p.rest) {
        return None;
    }

    // `**rest` needs no special case HERE: ingest models it as an
    // ordinary trailing positional (`library_class.rs`), so it raises
    // the positional count and the excess test below declines on its
    // own. That the desugar is therefore already correct holds only
    // while the `**rest` slot is the FIRST unfilled one. An optional
    // keyword beside it is also flattened to a positional, and the
    // bundle then lands in THAT slot — see `lower::kwrest_forward`,
    // which owns the case this pass declines.
    let positional = params.iter().filter(|p| !p.keyword && !p.rest).count();
    let keywords: Vec<(Symbol, Option<Expr>)> = params
        .iter()
        .filter(|p| p.keyword)
        .map(|p| (p.name.clone(), p.default.clone()))
        .collect();
    if keywords.is_empty() || args.len() != positional + 1 {
        return None;
    }
    // A keyword param carrying a default is Ruby's optional keyword; it
    // expands only when that default can be restated at the call site.
    let defaults_literal = keywords
        .iter()
        .all(|(_, d)| d.as_ref().is_none_or(is_literal));
    Some(ErasedSplat { keywords, defaults_literal })
}

/// A default that means the same thing at the call site as in the
/// callee: literals and empty collections. A constant may resolve
/// differently (or not at all) where the caller sits, and a call would
/// run in the wrong scope.
fn is_literal(expr: &Expr) -> bool {
    match &*expr.node {
        ExprNode::Lit { .. } => true,
        ExprNode::Hash { entries, .. } => entries.is_empty(),
        ExprNode::Array { elements, .. } => elements.is_empty(),
        _ => false,
    }
}

/// The callee's parameter list, walking the inheritance chain the way
/// `normalize_trailing_kwargs` does — including its `new` → `initialize`
/// hop, since Ruby auto-generates `new` to forward to the constructor.
fn callee_params<'s>(
    recv_ty: &Ty,
    method: &Symbol,
    sigs: &'s Signatures,
) -> Option<&'s Vec<Param>> {
    let Ty::Class { id, .. } = recv_ty else { return None };
    let lookup = if method.as_str() == "new" {
        Symbol::from("initialize")
    } else {
        method.clone()
    };
    let mut current = Some(id.clone());
    // Same cycle cap `normalize_trailing_kwargs` uses — a malformed
    // parent chain must not hang the compiler.
    for _ in 0..32 {
        let cid = current?;
        if let Some(params) = sigs.methods.get(&(cid.clone(), lookup.clone())) {
            return Some(params);
        }
        current = sigs.parents.get(&cid).cloned();
    }
    None
}

/// Evaluated once per keyword, so only expressions that are free of
/// side effects AND cheap qualify.
fn is_pure_read(expr: &Expr) -> bool {
    matches!(
        &*expr.node,
        ExprNode::Var { .. } | ExprNode::Ivar { .. } | ExprNode::Const { .. }
    )
}

fn sym_key(name: &Symbol, span: crate::span::Span) -> Expr {
    let mut e = Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: name.clone() } });
    e.ty = Some(Ty::Sym);
    e
}

/// `<hash>[:<key>]`, typed the same way `hash_method` types `[]`:
/// `value | nil`. An open (Var) value is stamped `Untyped` — the gradual
/// escape — so diagnose does not report a false `send_dispatch_failed`
/// for an index we just synthesized. Campfire's preview
/// `notification(badge: …, **params)` expands to `params[:title]` etc.;
/// with only `badge:`'s default typed, `params` stays `Hash[Var, Var]`
/// and a raw Var stamp on `[]` was misread as "no known method".
fn index(hash: &Expr, key: &Symbol, value_ty: Ty) -> Expr {
    let ty = match &value_ty {
        Ty::Var { .. } | Ty::Untyped => Ty::Untyped,
        other => Ty::Union {
            variants: vec![other.clone(), Ty::Nil],
        },
    };
    let mut e = Expr::new(
        hash.span,
        ExprNode::Send {
            recv: Some(hash.clone()),
            method: Symbol::from("[]"),
            args: vec![sym_key(key, hash.span)],
            block: None,
            parenthesized: false,
        },
    );
    e.ty = Some(ty);
    e
}

/// `<hash>.fetch(:<key>, <default>)`, typed as the hash's value type —
/// the read of an OPTIONAL keyword, with the callee's default standing
/// in for an absent key as Ruby's `**` would have it.
fn fetch(hash: &Expr, key: &Symbol, default: &Expr, value_ty: Ty) -> Expr {
    // `diagnose` treats an untyped send on a known receiver as
    // "no known method". The hash value is often untyped; the
    // default is the shape this read answers, the same rule
    // `hash_method` uses for a closed default.
    let ty = match default.ty.as_ref() {
        Some(default_ty) if !default_ty.is_open() && matches!(value_ty, Ty::Untyped | Ty::Var { .. }) => {
            default_ty.clone()
        }
        Some(default_ty) if !default_ty.is_open() => crate::analyze::union_of(value_ty, default_ty.clone()),
        _ if matches!(value_ty, Ty::Untyped | Ty::Var { .. }) => {
            default.ty.clone().unwrap_or(value_ty)
        }
        _ => value_ty,
    };
    let mut e = Expr::new(
        hash.span,
        ExprNode::Send {
            recv: Some(hash.clone()),
            method: Symbol::from("fetch"),
            args: vec![sym_key(key, hash.span), default.clone()],
            block: None,
            // Inside a keyword list, so the two arguments must be
            // fenced off from the keywords that follow.
            parenthesized: true,
        },
    );
    e.ty = Some(ty);
    e
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    super::residue_diagnostic(
        "kwsplat",
        "keyword-splat",
        expr.span,
        reason,
        format!(
            "`**hash` into a keyword-only callee left as a positional \
             argument ({reason}) — the call will raise ArgumentError"
        ),
    )
}
