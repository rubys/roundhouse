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
use crate::dialect::{MethodReceiver, ModelBodyItem};
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
    /// Superclass links, so a call landing on an inherited `initialize`
    /// still resolves.
    parents: HashMap<ClassId, ClassId>,
}

pub fn apply_kwsplat_expansion(app: &mut App) -> Vec<Diagnostic> {
    let sigs = collect_signatures(app);
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| rewrite(body, &sigs, &mut diags));
    apply_to_test_modules(app, &mut diags);
    diags
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
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else {
            return;
        };
        let Some(params) = helpers.get(method) else { return };
        let Some(splat) = erased_splat_against(args, params) else { return };
        splat
    };
    let ExprNode::Send { args, .. } = &mut *expr.node else {
        unreachable!("matched a Send above")
    };
    expand(args, splat, diags);
}

/// Every instance method an app class declares. Class-side methods are
/// skipped: a `Class.new(…)` call resolves to `initialize`, and no other
/// receiver shape this pass matches reaches a `def self.`.
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

    let Some(splat) = erased_splat(expr, sigs) else {
        return;
    };
    let ExprNode::Send { args, .. } = &mut *expr.node else {
        unreachable!("erased_splat matched a Send")
    };
    expand(args, splat, diags);
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
    let Some((read, literal)) = splat_shape(hash) else {
        diags.push(residue(
            hash,
            "the splatted expression is not a local, ivar or constant read",
        ));
        return;
    };
    let (read, literal): (Expr, Vec<(Expr, Expr)>) = (read.clone(), literal.to_vec());

    let hash = args.pop().expect("checked above");
    let value_ty = match &read.ty {
        Some(Ty::Hash { value, .. }) => (**value).clone(),
        _ => Ty::Untyped,
    };
    // A keyword the literal names takes the literal's value — evaluated
    // once, as Ruby's `**` would; the rest are read off the bundle, an
    // optional one with its declared default in hand.
    let entries = splat
        .keywords
        .iter()
        .map(|(kw, default)| {
            let value = literal
                .iter()
                .find(|(k, _)| sym_of(k).is_some_and(|k| k == kw))
                .map(|(_, v)| v.clone())
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
/// keywords it does NOT name are indexed off `h`. A key that is not a
/// Symbol literal, or a merge whose argument is not a literal, is a
/// bundle this pass cannot read, and declines.
fn splat_shape(expr: &Expr) -> Option<(&Expr, &[(Expr, Expr)])> {
    if is_pure_read(expr) {
        return Some((expr, &[]));
    }
    let ExprNode::Send { recv: Some(recv), method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    if method.as_str() != "merge" || args.len() != 1 || !is_pure_read(recv) {
        return None;
    }
    let ExprNode::Hash { entries, .. } = &*args[0].node else { return None };
    if !entries.iter().all(|(k, _)| sym_of(k).is_some()) {
        return None;
    }
    Some((recv, entries.as_slice()))
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

/// `<hash>[:<key>]`, typed as the hash's value type.
fn index(hash: &Expr, key: &Symbol, value_ty: Ty) -> Expr {
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
    e.ty = Some(value_ty);
    e
}

/// `<hash>.fetch(:<key>, <default>)`, typed as the hash's value type —
/// the read of an OPTIONAL keyword, with the callee's default standing
/// in for an absent key as Ruby's `**` would have it.
fn fetch(hash: &Expr, key: &Symbol, default: &Expr, value_ty: Ty) -> Expr {
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
    e.ty = Some(value_ty);
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
