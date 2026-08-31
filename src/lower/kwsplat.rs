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
//! ## The three guards, and why each one fails closed
//!
//! - **Every keyword parameter must be required.** With an optional
//!   keyword, `k: h[:k]` passes `nil` for an absent key where Ruby would
//!   have used the declared default — a silently different value. There
//!   is no static way to know which keys `h` holds.
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
    diags
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
    /// Keyword parameter names, in declaration order.
    keywords: Vec<Symbol>,
    /// False when any keyword carries a default — the pass may not
    /// expand it, but still ledgers it.
    all_required: bool,
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
    let hash = args.last().expect("erased_splat matched a trailing arg");

    if !splat.all_required {
        diags.push(residue(hash, "callee declares optional keyword parameters"));
        return;
    }
    if !is_pure_read(hash) {
        diags.push(residue(
            hash,
            "the splatted expression is not a local, ivar or constant read",
        ));
        return;
    }

    let hash = args.pop().expect("checked above");
    let value_ty = match &hash.ty {
        Some(Ty::Hash { value, .. }) => (**value).clone(),
        _ => Ty::Untyped,
    };
    let entries = splat
        .keywords
        .iter()
        .map(|kw| (sym_key(kw, hash.span), index(&hash, kw, value_ty.clone())))
        .collect();
    let mut kwargs = Expr::new(hash.span, ExprNode::Hash { entries, kwargs: true });
    kwargs.ty = hash.ty.clone();
    args.push(kwargs);
}

/// Is `expr` a call whose trailing positional argument can only have
/// been written `**hash`? See the header for why the excess-argument
/// count is sufficient evidence.
fn erased_splat(expr: &Expr, sigs: &Signatures) -> Option<ErasedSplat> {
    let ExprNode::Send { recv: Some(recv), method, args, .. } = &*expr.node else {
        return None;
    };
    let last = args.last()?;
    // A literal keyword list already renders as keywords.
    if matches!(&*last.node, ExprNode::Hash { kwargs: true, .. }) {
        return None;
    }

    let params = callee_params(recv.ty.as_ref()?, method, sigs)?;

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
    let keywords: Vec<Symbol> = params
        .iter()
        .filter(|p| p.keyword)
        .map(|p| p.name.clone())
        .collect();
    if keywords.is_empty() || args.len() != positional + 1 {
        return None;
    }
    // A keyword param carrying a default is Ruby's optional keyword.
    let all_required = params.iter().all(|p| !p.keyword || p.default.is_none());
    Some(ErasedSplat { keywords, all_required })
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
