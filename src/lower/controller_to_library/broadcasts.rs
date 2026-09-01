//! `broadcast_<action>_to` in a CONTROLLER body.
//!
//! The third home these calls have. `lower::broadcast_calls` covers the
//! other two — a model's own body, and a concern module beside it — and
//! campfire writes the same Rails API from a controller:
//!
//! ```text
//! def broadcast_create_room(room)
//!   broadcast_prepend_to :rooms, target: :shared_rooms,
//!     partial: "users/sidebars/rooms/shared", locals: { room: room }
//! end
//! ```
//!
//! Emitted verbatim until now, which is an undefined method — those
//! actions raise in a real server, not just in a test. The model-side
//! rewriter can't be reused as-is because it resolves everything from
//! the owning MODEL: the stream's record comes from a `belongs_to`, and
//! the payload is always that model's own partial. A controller has no
//! owner. What it has instead is TYPES — analyze has stamped
//! `Ty::Class` on the streamable expressions by the time controllers
//! lower — and an explicit `partial:`/`html:` naming the payload.
//!
//! What is shared is what must not drift: `broadcasts::stream_name`
//! spells the stream (the subscribe side spells it the same way, and a
//! disagreement is silent), and `partial_view_call` binds the partial
//! (the same binding `render partial:` gets).
//!
//! Anything not modeled DECLINES with a ledger line rather than
//! guessing — a broadcast to a stream name we invented reaches nobody
//! and looks like it worked.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::lower::broadcasts::Streamable;
use crate::span::Span;
use crate::ty::Ty;

use super::rewrites::partial_view_call_with_record;
use super::util::map_expr;

/// Rewrite every recognized broadcast call in a controller method body.
pub(super) fn rewrite_broadcast_to(
    expr: &Expr,
    module_name: Option<&str>,
    partials: &super::PartialMap,
) -> Expr {
    map_expr(expr, &|e| try_rewrite(e, module_name, partials))
}

fn try_rewrite(
    e: &Expr,
    module_name: Option<&str>,
    partials: &super::PartialMap,
) -> Option<Expr> {
    let ExprNode::Send { recv, method, args, block: None, .. } = &*e.node else {
        return None;
    };
    let action = action_of(method.as_str())?;
    let (positional, opts) = split_trailing_kwargs(args);
    if positional.is_empty() {
        return decline(e.span, "broadcast with no stream");
    }

    // The streamables, left to right: `:rooms` is its own text, a
    // record contributes `<singular>_<id>` with the id read at run
    // time. A streamable we cannot name declines the whole call.
    let mut parts: Vec<Streamable> = Vec::new();
    for arg in positional {
        parts.push(streamable(arg).or_else(|| {
            decline::<Streamable>(e.span, "streamable is not a literal or a nameable record")
        })?);
    }
    let stream = crate::lower::broadcasts::stream_name(&parts);

    let target = match opts.iter().find(|(k, _)| k.as_str() == "target") {
        Some((_, value)) => dom_target(value, e.span)?,
        // Rails defaults an omitted target to the broadcasting
        // record's dom_id. That needs a receiver to name.
        None => {
            let recv = recv.as_ref()?;
            let singular = record_singular(recv)?;
            record_dom_id(&singular, recv, e.span)
        }
    };

    let html = match action {
        Act::Remove => None,
        _ => Some(payload(&opts, recv.as_ref(), module_name, partials, e.span)?),
    };

    for (key, _) in &opts {
        match key.as_str() {
            "target" | "partial" | "locals" | "html" | "attributes" => {}
            other => return decline(e.span, &format!("broadcast option `{other}:`")),
        }
    }

    // `attributes: { maintain_scroll: true }` rides on the turbo-stream
    // ELEMENT. Rendered HERE, to the attribute text the element carries,
    // rather than threaded as a hash: the value is a literal at every
    // call site, so the compiler already knows the answer — and a String
    // is the one type all nine `Broadcasts` twins can hold without a
    // per-target hash-to-markup renderer that would be nine chances to
    // disagree.
    let attributes = match opts.iter().find(|(k, _)| k.as_str() == "attributes") {
        Some((_, value)) => element_attributes(value, e.span)?,
        None => String::new(),
    };

    Some(broadcasts_call(action, stream, target, html, &attributes, e.span))
}

/// `{ maintain_scroll: true }` → ` maintain_scroll="true"`, the text
/// turbo-rails' `tag.turbo_stream(template, **attributes, action:,
/// target:)` writes ahead of `action`/`target`.
///
/// Measured against ActionView 8.1's `TagBuilder`, not assumed:
///
///   * the key is written AS SPELLED — `tag_option` dasherizes nothing,
///     and campfire's own `maintain_scroll_controller.js` reads
///     `hasAttribute("maintain_scroll")`, so a helpfully-dasherized name
///     would reach the client and do nothing;
///   * the value is `to_s` then HTML-escaped (`true` → `"true"`,
///     `false` → `"false"` — `maintain_scroll` is not one of Rails'
///     BOOLEAN_ATTRIBUTES, so `false` is written, not dropped);
///   * a `nil` value omits the attribute entirely.
///
/// LITERALS ONLY. A computed value would have to be composed at run time
/// on every target; declining leaves the call as written and files the
/// reason, which is the same trade the rest of this file makes.
fn element_attributes(value: &Expr, span: Span) -> Option<String> {
    let ExprNode::Hash { entries, .. } = &*value.node else {
        return decline(span, "attributes: is not a literal hash");
    };
    let mut out = String::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            return decline(span, "attributes: key is not a symbol");
        };
        let text = match &*v.node {
            ExprNode::Lit { value: Literal::Nil } => continue,
            ExprNode::Lit { value: Literal::Bool { value } } => value.to_string(),
            ExprNode::Lit { value: Literal::Int { value } } => value.to_string(),
            ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
            ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
            _ => return decline(span, "attributes: value is not a literal"),
        };
        out.push(' ');
        out.push_str(key.as_str());
        out.push_str("=\"");
        out.push_str(&crate::lower::view_to_library::html_escape_fold(&text));
        out.push('"');
    }
    Some(out)
}

/// The payload: `html:` verbatim, `partial:` bound the way a
/// `render partial:` is bound, or — with neither — the receiver's own
/// partial, which is what the model-side rewriter renders.
///
/// Only the renders THIS lowering synthesizes go through
/// `broadcast_render` (Rails hands them to its session-less broadcast
/// renderer, so no CSRF token input). An app-supplied `html:` was
/// rendered by the app's own `render_to_string` inside the request —
/// with the session, token and all — and Rails broadcasts those bytes
/// untouched, so we pass the expression through unwrapped.
fn payload(
    opts: &[(Symbol, Expr)],
    recv: Option<&Expr>,
    module_name: Option<&str>,
    partials: &super::PartialMap,
    span: Span,
) -> Option<Expr> {
    if let Some((_, html)) = opts.iter().find(|(k, _)| k.as_str() == "html") {
        return Some(html.clone());
    }
    if let Some((_, p)) = opts.iter().find(|(k, _)| k.as_str() == "partial") {
        let ExprNode::Lit { value: Literal::Str { value: name } } = &*p.node else {
            return decline(span, "partial: is not a literal");
        };
        let locals = match opts.iter().find(|(k, _)| k.as_str() == "locals") {
            Some((_, l)) => hash_entries(l)?,
            None => Vec::new(),
        };
        return partial_view_call_with_record(name, &locals, recv, module_name, partials, span)
            .map(wrap_broadcast_render)
            .or_else(|| decline(span, &format!("partial `{name}` has no def-site contract")));
    }
    // No payload option: render the receiver's own partial, the
    // convention the model-side broadcast uses.
    let recv = recv.or_else(|| decline(span, "broadcast with no payload and no receiver"))?;
    let singular = record_singular(recv)
        .or_else(|| decline(span, "broadcast payload receiver is not a nameable record"))?;
    let plural_camel = crate::naming::camelize(&crate::naming::pluralize_snake(&singular));
    Some(wrap_broadcast_render(Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("Views"), Symbol::from(plural_camel)] },
            )),
            method: Symbol::from(singular),
            args: vec![recv.clone()],
            block: None,
            parenthesized: true,
        },
    )))
}

/// `target: :shared_rooms` → `"shared_rooms"`; `target: [@room, :list]`
/// → `"list_room_#{@room.id}"` (Rails' `dom_id(record, prefix)`, prefix
/// first).
fn dom_target(value: &Expr, span: Span) -> Option<Expr> {
    match &*value.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(lit_str(value.as_str(), span)),
        ExprNode::Lit { value: Literal::Str { value } } => Some(lit_str(value, span)),
        // `target: "boosts_message_#{@boost.message.client_message_id}"`
        // — an id the app spells itself. Already a String; nothing to
        // resolve, and rebuilding it would only be a chance to differ.
        ExprNode::StringInterp { .. } => Some(value.clone()),
        ExprNode::Array { elements, .. } => {
            let [record, prefix] = elements.as_slice() else {
                return decline(span, "target: array is not [record, prefix]");
            };
            let prefix = literal_text(prefix)
                .or_else(|| decline(span, "target: prefix is not a literal"))?;
            // `record_singular` still gates (only a nameable record
            // qualifies); the STRING comes from the synthesized
            // identity methods, so STI prefixes and `to_key`-keyed
            // rows spell what the pages spell — the same correction
            // model_to_library/broadcasts.rs carries.
            record_singular(record)
                .or_else(|| decline(span, "target: record is not a nameable record"))?;
            Some(Expr::new(
                span,
                ExprNode::StringInterp {
                    parts: vec![
                        crate::expr::InterpPart::Text { value: format!("{prefix}_") },
                        crate::expr::InterpPart::Expr {
                            expr: dom_identity_call(record.clone(), "dom_prefix"),
                        },
                        crate::expr::InterpPart::Text { value: "_".to_string() },
                        crate::expr::InterpPart::Expr {
                            expr: dom_identity_call(record.clone(), "dom_record_key"),
                        },
                    ],
                },
            ))
        }
        _ => decline(span, "target: is not a literal or [record, prefix]"),
    }
}

/// `"#{record.dom_prefix()}_#{record.dom_record_key()}"` — the
/// per-record DOM id, through the synthesized identity methods (see
/// the note above).
fn record_dom_id(_singular: &str, record: &Expr, span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::StringInterp {
            parts: vec![
                crate::expr::InterpPart::Expr {
                    expr: dom_identity_call(record.clone(), "dom_prefix"),
                },
                crate::expr::InterpPart::Text { value: "_".to_string() },
                crate::expr::InterpPart::Expr {
                    expr: dom_identity_call(record.clone(), "dom_record_key"),
                },
            ],
        },
    )
}

/// `ViewHelpers.broadcast_render(ViewHelpers.begin_broadcast_render,
/// <render>)` — the runtime pair brackets the render so
/// `csrf_token_hidden_input` omits the token input, matching Rails'
/// session-less broadcast renderer (a broadcast here runs inside the
/// triggering request, so the session would otherwise be at hand).
///
/// TWO PLAIN CALLS, NOT A BLOCK OR A PROC: left-to-right argument
/// evaluation raises the flag (first argument) before the render
/// (second argument) runs, on every target. A block dissolves on the
/// AOT lane when it captures into a heap poly proc (matz/spinel#4245),
/// and a proc argument needs `.call` + function-type arms in all nine
/// target emitters the runtime file lowers through.
fn wrap_broadcast_render(html: Expr) -> Expr {
    let vh_const = || {
        Expr::new(
            crate::span::Span::synthetic(),
            ExprNode::Const {
                path: vec![
                    Symbol::from("ActionView"),
                    Symbol::from("ViewHelpers"),
                ],
            },
        )
    };
    let mut armed = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Send {
            recv: Some(vh_const()),
            method: Symbol::from("begin_broadcast_render"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );
    armed.ty = Some(Ty::Str);
    let mut call = Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Send {
            recv: Some(vh_const()),
            method: Symbol::from("broadcast_render"),
            args: vec![armed, html],
            block: None,
            parenthesized: true,
        },
    );
    call.ty = Some(Ty::Str);
    call
}

/// A parenthesized zero-arg call to a synthesized dom-identity method
/// (`dom_prefix` / `dom_record_key`).
fn dom_identity_call(recv: Expr, name: &str) -> Expr {
    Expr::new(
        crate::span::Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from(name),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    )
}

/// One streamable argument. A literal is its own text; anything the
/// analyzer typed as a model is that record.
fn streamable(arg: &Expr) -> Option<Streamable> {
    if let Some(text) = literal_text(arg) {
        return Some(Streamable::Literal(text));
    }
    Some(Streamable::Record {
        singular: record_singular(arg)?,
        id: read_id(arg.clone(), arg.span),
    })
}

/// The record singular an expression stands for — `room` in
/// `room_#{room.id}`.
///
/// TYPE FIRST: `Ty::Class` is the honest answer and the model-side
/// rewriter's `belongs_to` lookup is its equivalent. But a controller's
/// records mostly arrive as ivars a `before_action` filter assigned
/// (`@room`, set by `set_room`), and the body-typer works one method at
/// a time — it sees `@room` in `broadcast_update_room` with no
/// assignment in that body and leaves it `TyVar`. So the NAME is the
/// fallback, which is not a new liberty: `rewrite_redirect_to` resolves
/// `redirect_to @article` to `article_path` by exactly this reading,
/// and Rails apps name a record's ivar after its class because every
/// other Rails convention already assumes it.
///
/// Where the two disagree the cost is asymmetric — a wrong route helper
/// is a loud missing-method, a wrong stream name is SILENT (the
/// broadcast reaches nobody) — which is why the type wins whenever
/// analyze supplied one.
///
/// Deliberately not filtered against `app.models`: the model list would
/// have to be threaded through `LowerControllerOptions`, whose
/// defaulting wrappers every emitter but ruby goes through, so the
/// filter would make this pass silently ruby-only — the exact shape of
/// bug this file exists to fix.
fn record_singular(e: &Expr) -> Option<String> {
    if let Some(Ty::Class { id, .. }) = e.ty.as_ref() {
        return Some(crate::naming::snake_case(id.0.as_str()));
    }
    match &*e.node {
        ExprNode::Ivar { name } => Some(name.as_str().to_string()),
        ExprNode::Var { name, .. } => Some(name.as_str().to_string()),
        // A reader chain names the record with its LAST segment:
        // `@membership.user` is a user, `@boost.message.room` a room.
        ExprNode::Send { method, args, block: None, .. } if args.is_empty() => {
            Some(method.as_str().to_string())
        }
        _ => None,
    }
}

fn read_id(recv: Expr, span: Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

#[derive(Clone, Copy)]
enum Act {
    Replace,
    Append,
    Prepend,
    Remove,
}

fn action_of(method: &str) -> Option<Act> {
    match method {
        "broadcast_replace_to" => Some(Act::Replace),
        "broadcast_append_to" => Some(Act::Append),
        "broadcast_prepend_to" => Some(Act::Prepend),
        "broadcast_remove_to" => Some(Act::Remove),
        _ => None,
    }
}

impl Act {
    fn method(self) -> &'static str {
        match self {
            Act::Replace => "replace",
            Act::Append => "append",
            Act::Prepend => "prepend",
            Act::Remove => "remove",
        }
    }
}

fn broadcasts_call(
    action: Act,
    stream: Expr,
    target: Expr,
    html: Option<Expr>,
    attributes: &str,
    span: Span,
) -> Expr {
    let mut entries: Vec<(Expr, Expr)> = vec![
        (lit_sym("stream", span), stream),
        (lit_sym("target", span), target),
    ];
    // `payload` already bracketed synthesized renders in
    // `broadcast_render`; an app-supplied `html:` arrives here unwrapped
    // and stays that way.
    if let Some(h) = html {
        entries.push((lit_sym("html", span), h));
    }
    // Omitted when empty, so every broadcast that carries no custom
    // attribute emits exactly the call it emitted before — the runtime
    // parameter defaults to the same "".
    if !attributes.is_empty() {
        entries.push((lit_sym("attributes", span), lit_str(attributes, span)));
    }
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(Expr::new(
                span,
                ExprNode::Const { path: vec![Symbol::from("Broadcasts")] },
            )),
            method: Symbol::from(action.method()),
            args: vec![Expr::new(span, ExprNode::Hash { entries, kwargs: true })],
            block: None,
            parenthesized: true,
        },
    )
}

fn hash_entries(e: &Expr) -> Option<Vec<(Symbol, Expr)>> {
    let ExprNode::Hash { entries, .. } = &*e.node else { return None };
    Some(
        entries
            .iter()
            .filter_map(|(k, v)| match &*k.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some((value.clone(), v.clone())),
                _ => None,
            })
            .collect(),
    )
}

fn split_trailing_kwargs(args: &[Expr]) -> (&[Expr], Vec<(Symbol, Expr)>) {
    let Some(last) = args.last() else { return (args, Vec::new()) };
    let ExprNode::Hash { entries, kwargs: true } = &*last.node else {
        return (args, Vec::new());
    };
    let opts = entries
        .iter()
        .filter_map(|(k, v)| match &*k.node {
            ExprNode::Lit { value: Literal::Sym { value } } => Some((value.clone(), v.clone())),
            _ => None,
        })
        .collect();
    (&args[..args.len() - 1], opts)
}

fn literal_text(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

fn lit_str(value: &str, span: Span) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Str { value: value.to_string() } })
}

fn lit_sym(name: &str, span: Span) -> Expr {
    Expr::new(span, ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } })
}

/// Leave the call alone and file the reason — same contract as the
/// model-side rewriter's decline.
fn decline<T>(span: Span, what: &str) -> Option<T> {
    crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
        "broadcast",
        what,
        span,
        "controller broadcast call not lowered",
        format!("`{what}` is not modeled — the call is emitted as written"),
    ));
    None
}
