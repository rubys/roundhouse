//! Controller-body lowering — shared Phase 4c analysis.
//!
//! The four Phase-4c emitters (Rust, Crystal, Go, Elixir) each wanted
//! to match Ruby controller-body `Send` shapes and rewrite them into
//! a target-specific runtime call. The IR-match logic was identical;
//! only the rendering varied.
//!
//! This module exposes both halves of the shared piece:
//!
//! - **Predicates + walkers** (`split_public_private`,
//!   `walk_controller_ivars`, `is_query_builder_method`,
//!   `singularize_to_model`, `chain_target_class`, `is_params_expr`,
//!   `is_format_binding`) — building blocks each emitter can pull
//!   into its rendering pipeline.
//!
//! - **`SendKind` classifier** (`classify_controller_send`) — the
//!   lowered view of every Send shape each emitter cares about.
//!   Takes the raw `recv / method / args / block` and returns a
//!   tagged variant; the emitter's render table then produces
//!   target syntax. Unclassified Sends return `None` and fall
//!   through to the emitter's normal path (plain `recv.method(args)`
//!   rendering, self-dispatch, etc.).
//!
//! Variants live here when the shape appears in at least three of
//! the four emitters — validation that they're shape-shaped, not
//! target-shaped. Target-specific rewrites (Elixir's struct-method-
//! to-Module-function conversion) stay in the emitter.

use std::collections::BTreeSet;

use crate::dialect::{Action, Controller, ControllerBodyItem};
use crate::expr::{Expr, ExprNode, LValue};
use crate::ident::Symbol;
use crate::naming;

/// Walk a controller's source-ordered body, partitioning actions into
/// those before the `private` marker vs. those after. Filters and
/// Unknown class-body calls are informational-only for emit and get
/// dropped; PrivateMarker is consumed as the partition point.
pub fn split_public_private(c: &Controller) -> (Vec<Action>, Vec<Action>) {
    let mut pubs = Vec::new();
    let mut privs = Vec::new();
    let mut seen_private = false;
    for item in &c.body {
        match item {
            ControllerBodyItem::PrivateMarker { .. } => seen_private = true,
            ControllerBodyItem::Action { action, .. } => {
                if seen_private {
                    privs.push(action.clone());
                } else {
                    pubs.push(action.clone());
                }
            }
            _ => {}
        }
    }
    (pubs, privs)
}

/// Walk an action body collecting every ivar it touches. Returns two
/// sets (both deterministic):
///
/// - `assigned`: ivar names that appear on the LHS of an assignment
///   at some point in the body.
/// - `referenced`: ivar names in first-use order — every read *or*
///   write registers here. Used by the Rust emitter to compute
///   "referenced but never assigned" (the Rails `before_action`
///   filter would set these in the real runtime; Phase 4c primes them
///   with defaults).
///
/// Callers that only need the referenced list (Crystal, Go) pull
/// that half and ignore `assigned`.
pub fn walk_controller_ivars(body: &Expr) -> WalkedIvars {
    let mut out = WalkedIvars::default();
    walk(body, &mut out);
    out
}

#[derive(Default, Debug, Clone)]
pub struct WalkedIvars {
    /// ivar names that appear as the LHS of an assignment.
    pub assigned: BTreeSet<Symbol>,
    /// ivar names in first-use order across the body (read or write).
    pub referenced: Vec<Symbol>,
    /// Fast-lookup mirror of `referenced` to keep insertions O(log n)
    /// without losing ordering.
    seen: BTreeSet<Symbol>,
}

impl WalkedIvars {
    pub fn ivars_read_without_assign(&self) -> Vec<Symbol> {
        self.referenced
            .iter()
            .filter(|n| !self.assigned.contains(*n))
            .cloned()
            .collect()
    }
}

fn walk(e: &Expr, out: &mut WalkedIvars) {
    match &*e.node {
        ExprNode::Ivar { name } => {
            if out.seen.insert(name.clone()) {
                out.referenced.push(name.clone());
            }
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            out.assigned.insert(name.clone());
            if out.seen.insert(name.clone()) {
                out.referenced.push(name.clone());
            }
            walk(value, out);
        }
        ExprNode::Assign { value, .. } => walk(value, out),
        ExprNode::Seq { exprs } => {
            for child in exprs {
                walk(child, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk(cond, out);
            walk(then_branch, out);
            walk(else_branch, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk(left, out);
            walk(right, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                walk(r, out);
            }
            for a in args {
                walk(a, out);
            }
            if let Some(b) = block {
                walk(b, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk(k, out);
                walk(v, out);
            }
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk(el, out);
            }
        }
        ExprNode::Lambda { body, .. } => walk(body, out),
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let crate::expr::InterpPart::Expr { expr } = p {
                    walk(expr, out);
                }
            }
        }
        _ => {}
    }
}

/// Query-builder method names that don't have a Phase 4c runtime.
/// Chains containing any of these collapse to an empty collection of
/// the chain's target model type at emit time. The set is the same on
/// every Phase-4c target — shape-shaped, not target-shaped.
///
/// `all` lives here too: the generated model has no `all` method, and
/// without this collapse each controller calling `Model.all` would
/// fail to compile on the typed targets.
pub fn is_query_builder_method(method: &str) -> bool {
    matches!(
        method,
        "all"
            | "includes"
            | "order"
            | "where"
            | "group"
            | "limit"
            | "offset"
            | "joins"
            | "distinct"
            | "select"
            | "pluck"
            | "first"
            | "last"
    )
}

/// Resolve a HasMany association name to its target model class.
/// `"comments"` → `"Comment"` iff `Comment` is in `known_models`.
///
/// Used by the `.build(hash)` / `.create(hash)` / `<assoc>.find(x)`
/// rewrites in every Phase-4c emitter — they all need to default-
/// construct the target, and the target's name falls out of
/// singularising the method name on the association chain.
pub fn singularize_to_model(assoc: &str, known_models: &[Symbol]) -> Option<Symbol> {
    let class = naming::singularize_camelize(assoc);
    known_models
        .iter()
        .find(|m| m.as_str() == class)
        .cloned()
}

/// Walk a chain of `Send`s left until hitting a `Const { path }`.
/// Returns the final path segment (the presumed class name) when it's
/// a known model. Used to pick the element type for
/// `Vec::<T>::new()` / `[] of T` / `[]*T{}` chain punts.
pub fn chain_target_class(e: &Expr, known_models: &[Symbol]) -> Option<Symbol> {
    let mut cur = e;
    loop {
        match &*cur.node {
            ExprNode::Const { path } => {
                let class = path.last()?;
                return known_models
                    .iter()
                    .find(|m| m.as_str() == class.as_str())
                    .cloned();
            }
            ExprNode::Send { recv: Some(r), .. } => cur = r,
            _ => return None,
        }
    }
}

/// True when an expression references the implicit `params` object —
/// a bare `Send { recv: None, method: "params", args: [] }`. Used by
/// the `params.expect(...)` / `params[k]` rewrites in every
/// Phase-4c emitter.
pub fn is_params_expr(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Send { recv: None, method, args, .. }
            if method.as_str() == "params" && args.is_empty()
    )
}

/// True when an expression is the block parameter bound by
/// `respond_to do |format|` — today just the local `format` var. Used
/// to disambiguate `format.html { ... }` inside a respond_to block
/// from any unrelated `x.html` call outside.
pub fn is_format_binding(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Var { name, .. } if name.as_str() == "format"
    )
}

// -- SendKind classifier --------------------------------------------
//
// Each variant names a controller-body Send shape that at least three
// of the four Phase-4c emitters (Rust / Crystal / Go / Elixir) handle.
// The classifier extracts the *intent* from the IR; the per-target
// emitter's render table produces the target syntax. Variants carry
// references into the original `Send` so the emitter can keep using
// its own ctx to render args/recv/block — nothing is pre-rendered in
// the classifier.
//
// `method: &'a str` on `BangStrip` is the *stripped* name
// (`"destroy"` for a `"destroy!"` input) since the emitters that
// strip all want that form; Crystal — which keeps the bang — bypasses
// this variant and renders through its own path.

/// Classified shape of a controller-body `Send`. `None` from
/// `classify_controller_send` means "fall through to the emitter's
/// normal Send rendering."
#[derive(Debug)]
pub enum SendKind<'a> {
    // HTTP surface — bare calls with no receiver, no block.
    /// `render(args...)` bare.
    Render { args: &'a [Expr] },
    /// `redirect_to(args...)` bare.
    RedirectTo { args: &'a [Expr] },
    /// `head(status)` bare.
    Head { args: &'a [Expr] },

    // respond_to + format.* routing.
    /// `respond_to do |format| body end` — `body` is the unwrapped
    /// block body (Lambda layer already peeled).
    RespondToBlock { body: &'a Expr },
    /// `format.html { body }` — `body` is the unwrapped block body.
    FormatHtml { body: &'a Expr },
    /// `format.json { … }` — contents intentionally dropped per
    /// Phase 4c's JSON-branch-is-TODO convention.
    FormatJson,

    // Params surface.
    /// Bare `params`.
    ParamsAccess,
    /// `params.expect(args...)`.
    ParamsExpect { args: &'a [Expr] },
    /// `params[key]`.
    ParamsIndex { key: &'a Expr },

    // Model class methods.
    /// `Model.new` / `Model.new(anything)` — args dropped by every
    /// emitter (generated models have no keyword/positional ctor).
    ModelNew { class: Symbol },
    /// `Model.find(id)` — the class method, returning a nullable.
    /// Each emitter appends its own unwrap flavour.
    ModelFind { class: Symbol, id: &'a Expr },

    // Association / query chain shapes.
    /// `<assoc>.find(x)` / `<assoc>.build(h)` / `<assoc>.create(h)`
    /// on a non-Const receiver (the outer method's `recv` is a
    /// Send whose method name singularizes to a known model).
    /// Every emitter renders this as a zero-value of `target`.
    AssocLookup { target: Symbol, outer_method: &'a str },
    /// Unsupported query-builder chain (`.all`/`.order`/`.where`/…).
    /// Target class from chain walk; `None` when the chain's head
    /// isn't a known model.
    QueryChain { target: Option<Symbol> },

    /// Bare `*_path` / `*_url` — Rails URL helpers. No runtime.
    PathOrUrlHelper,

    /// `.destroy!` / `.save!` / `.update!` — three of four targets
    /// (Rust, Go, Elixir) strip the bang; Crystal accepts it and
    /// bypasses this variant.
    BangStrip {
        recv: &'a Expr,
        stripped_method: &'a str,
        args: &'a [Expr],
    },

    /// `x.update(...)` on a non-Const receiver — no runtime yet; all
    /// four emitters punt to a boolean stub.
    InstanceUpdate,
}

/// Classify a controller-body `Send` into a shared `SendKind` variant.
/// Returns `None` for shapes that don't match any shared pattern;
/// the caller falls through to its normal Send rendering (self-
/// dispatch, plain `recv.method(args)`, etc.).
///
/// This is the IR side of the Phase 4c shared-analysis work. The
/// four emitters each had a near-identical match-and-rewrite pass;
/// this function captures it once, and the emitters become render
/// tables over `SendKind`.
pub fn classify_controller_send<'a>(
    recv: Option<&'a Expr>,
    method: &'a str,
    args: &'a [Expr],
    block: Option<&'a Expr>,
    known_models: &[Symbol],
) -> Option<SendKind<'a>> {
    // Bare `params` — a zero-arg, no-block Send with recv=None.
    if recv.is_none() && method == "params" && args.is_empty() && block.is_none() {
        return Some(SendKind::ParamsAccess);
    }

    // `params.expect(...)` and `params[k]` — recv must match the
    // bare-`params` shape.
    if let Some(r) = recv {
        if is_params_expr(r) {
            if method == "expect" {
                return Some(SendKind::ParamsExpect { args });
            }
            if method == "[]" && !args.is_empty() {
                return Some(SendKind::ParamsIndex { key: &args[0] });
            }
        }
    }

    // `respond_to do |format| ... end`. Unwrap one Lambda layer so
    // the emitter sees the block body directly.
    if recv.is_none() && method == "respond_to" && block.is_some() {
        let body = unwrap_lambda(block.unwrap());
        return Some(SendKind::RespondToBlock { body });
    }

    // `format.html { body }` / `format.json { body }`.
    if let Some(r) = recv {
        if is_format_binding(r) {
            match method {
                "html" => {
                    if let Some(b) = block {
                        let body = unwrap_lambda(b);
                        return Some(SendKind::FormatHtml { body });
                    }
                }
                "json" => return Some(SendKind::FormatJson),
                _ => {}
            }
        }
    }

    // Bare `render` / `redirect_to` / `head`.
    if recv.is_none() && block.is_none() {
        match method {
            "render" => return Some(SendKind::Render { args }),
            "redirect_to" => return Some(SendKind::RedirectTo { args }),
            "head" => return Some(SendKind::Head { args }),
            _ => {}
        }
    }

    // Bare `*_path` / `*_url` — Rails URL helper.
    if recv.is_none()
        && block.is_none()
        && (method.ends_with("_path") || method.ends_with("_url"))
    {
        return Some(SendKind::PathOrUrlHelper);
    }

    // `Model.new` / `Model.new(...)` and `Model.find(id)` — class
    // method calls on a known model.
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            if let Some(class) = path.last() {
                if let Some(resolved) = known_models.iter().find(|m| *m == class) {
                    match method {
                        "new" => {
                            return Some(SendKind::ModelNew {
                                class: resolved.clone(),
                            });
                        }
                        "find" => {
                            if let Some(id) = args.first() {
                                return Some(SendKind::ModelFind {
                                    class: resolved.clone(),
                                    id,
                                });
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    // Query-builder chains — `all`/`includes`/`order`/`where`/... on
    // anything. Target model is the chain's Const head.
    if is_query_builder_method(method) {
        let target = recv.and_then(|r| chain_target_class(r, known_models));
        return Some(SendKind::QueryChain { target });
    }

    // `<assoc>.find(x)` / `.build(h)` / `.create(h)` on a Send recv
    // whose inner method singularizes to a known model.
    if matches!(method, "find" | "build" | "create") {
        if let Some(r) = recv {
            if let ExprNode::Send {
                recv: Some(_),
                method: assoc_method,
                args: inner_args,
                ..
            } = &*r.node
            {
                if inner_args.is_empty() {
                    if let Some(target) =
                        singularize_to_model(assoc_method.as_str(), known_models)
                    {
                        return Some(SendKind::AssocLookup {
                            target,
                            outer_method: method,
                        });
                    }
                }
            }
        }
    }

    // `.destroy!` / `.save!` / `.update!` — three of four emitters
    // strip the bang. Crystal bypasses this variant and renders the
    // bang-suffixed name directly.
    if let Some(r) = recv {
        if method == "destroy!" || method == "save!" || method == "update!" {
            let stripped = &method[..method.len() - 1];
            return Some(SendKind::BangStrip {
                recv: r,
                stripped_method: stripped,
                args,
            });
        }
    }

    // `x.update(...)` on a non-Const receiver.
    if method == "update" {
        if let Some(r) = recv {
            if !matches!(&*r.node, ExprNode::Const { .. }) {
                return Some(SendKind::InstanceUpdate);
            }
        }
    }

    None
}

/// Peel one `ExprNode::Lambda` layer — Ruby `do ... end` / `{ ... }`
/// ingests as a `Lambda` in the IR, but for emit purposes each block
/// is rendered as its body's statements, not as a lambda.
fn unwrap_lambda(e: &Expr) -> &Expr {
    match &*e.node {
        ExprNode::Lambda { body, .. } => body,
        _ => e,
    }
}
