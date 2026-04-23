//! `SendKind` — the shared classifier that collapses every
//! controller-body Send shape the four Phase-4c emitters (Rust,
//! Crystal, Go, Elixir) care about into one tagged enum.
//!
//! Variants carry references into the original `Send` (not
//! pre-rendered strings) so each emitter keeps using its own ctx
//! to render args/recv/block. Unclassified Sends return `None` and
//! fall through to the emitter's normal path.
//!
//! A variant earns its place here when the shape appears in at least
//! three of the four emitters — that's the threshold for shape-
//! shaped vs. target-shaped. Target-specific rewrites (Elixir's
//! struct-method-to-Module-function conversion) stay in the emitter.

use crate::expr::{Expr, ExprNode};
use crate::ident::Symbol;

use super::util::{chain_target_class, is_format_binding, is_params_expr, singularize_to_model, unwrap_lambda};

// `is_query_builder_method` lives in `crate::catalog` (a
// runtime-capability concern); we consult it directly rather than
// re-importing from the parent.
use crate::catalog::is_query_builder_method;

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
    /// Query-builder chain — `.all`/`.order`/`.where`/`.includes`/...
    /// on a model class. Emitters render the chain by composing
    /// the outer `method` over the (recursively rendered) `recv`.
    /// `target` is the chain's head class when known.
    ///
    /// - `method`: the outermost call (e.g. "order").
    /// - `args`: args to that outer call (e.g. `[{created_at:
    ///   :desc}]` for `order(created_at: :desc)`).
    /// - `recv`: the chain's receiver — either the head class
    ///   (Const) or an inner query-builder Send. Emitters render
    ///   this first, then apply the outer method.
    QueryChain {
        target: Option<Symbol>,
        method: &'a str,
        args: &'a [Expr],
        recv: Option<&'a Expr>,
    },

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
        return Some(SendKind::QueryChain {
            target,
            method,
            args,
            recv,
        });
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
