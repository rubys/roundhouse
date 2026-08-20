//! View-helper call construction. Translates classified view-helper
//! kinds (`link_to`, `dom_id`, `pluralize`, …) into spinel-shape
//! `ViewHelpers.*` / `RouteHelpers.*` / `Inflector.*` Sends, and
//! handles URL-position argument lowering.

use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::naming::singularize;
use crate::span::Span;

use crate::lower::view::{
    classify_nested_url_element, classify_view_url_arg, NestedUrlElement, ViewHelperKind,
    ViewUrlArg,
};

use super::attr_parts::{
    append_attr_parts, default_form_class, default_method_sym, lit_str_coerce, string_interp,
    take_opt,
};
use super::walker::rewrite_helpers_in_expr;
use super::{
    bare_record_name, inflector_call, lit_str, lit_sym, route_helpers_call, send, var_ref,
    view_helpers_call, ViewCtx,
};

/// `turbo_stream.<action>(target[, record])` → `Broadcasts
/// .turbo_stream_fragment("<action>", <target>, <html>)`.
///
/// The markup itself stays in each target's hand-written `Broadcasts`
/// (the same composer the model-side `broadcast_append_to` path uses),
/// so a change to the `<turbo-stream>` element has one owner rather than
/// a second copy here.
///
/// TARGET: a bare record is its `dom_id`, which is what Rails does — a
/// Symbol or String is the id verbatim, and anything else (typically an
/// explicit `dom_id(...)`) is already a String.
///
/// HTML: a bare record renders ITS partial, by the same dir/singular
/// convention `render @record` uses — `@message` → `Views::Messages
/// .message(message)`. `remove` carries no `<template>`, so it gets "".
/// Is this receiver the `turbo_stream` builder? Used to tell "not a
/// turbo_stream call at all" from "a turbo_stream call we don't lower",
/// so only the latter files a residue line.
pub(super) fn is_turbo_stream_builder(recv: Option<&Expr>) -> bool {
    match recv.map(|r| &*r.node) {
        Some(ExprNode::Send { recv: None, method, args, block: None, .. }) => {
            method.as_str() == "turbo_stream" && args.is_empty()
        }
        Some(ExprNode::Var { name, .. }) => name.as_str() == "turbo_stream",
        _ => false,
    }
}

/// The `<turbo-stream target=…>` value for a `turbo_stream.<action>`
/// call's first argument. Shared by all three spellings — positional,
/// option-hash and block — so they cannot disagree about what a bare
/// record means.
///
/// A bare record is its `dom_id`, which is what Rails does; a Symbol is
/// the id verbatim; anything else (typically an explicit `dom_id(...)`)
/// is already a String.
pub(super) fn turbo_stream_target(target: &Expr, ctx: &ViewCtx) -> Expr {
    use crate::expr::Literal;
    match &*target.node {
        ExprNode::Lit { value: Literal::Sym { value } } => lit_str(value.as_str().to_string()),
        ExprNode::Var { .. } | ExprNode::Ivar { .. } => {
            view_helpers_call("dom_id", vec![rewrite_helpers_in_expr(target, ctx)])
        }
        _ => rewrite_helpers_in_expr(target, ctx),
    }
}

/// `Broadcasts.turbo_stream_fragment("<action>", <target>, <html>)`.
/// The markup lives in each target's hand-written `Broadcasts` — the
/// same composer the model-side `broadcast_append_to` path uses — so a
/// change to the `<turbo-stream>` element has one owner.
pub(super) fn turbo_stream_fragment_call(action: &str, target: Expr, html: Expr) -> Expr {
    send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("Broadcasts")] },
        )),
        "turbo_stream_fragment",
        vec![lit_str(action.to_string()), target, html],
        None,
        true,
    )
}

pub(super) fn emit_turbo_stream_fragment(
    ts: &crate::lower::view::TurboStreamCall<'_>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    use crate::naming::pluralize_snake;

    let record_name = |e: &Expr| match &*e.node {
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.as_str().to_string()),
        _ => None,
    };

    let target = turbo_stream_target(ts.target, ctx);

    let html = match ts.content {
        None => lit_str(String::new()),
        Some(content) => {
            // Only the render-this-record form. Anything else (a literal
            // string, a nested call) would need the partial machinery a
            // `render` call site gets, so decline and leave the source
            // shape for the residue ledger.
            let name = record_name(content)?;
            let partial = format!("{}/{}", pluralize_snake(&name), name);
            super::partial::named_partial_call(
                &partial,
                Some(&rewrite_helpers_in_expr(content, ctx)),
                None,
                ctx,
            )?
        }
    };

    Some(turbo_stream_fragment_call(ts.action, target, html))
}

pub(super) fn emit_view_helper_call(kind: &ViewHelperKind<'_>, ctx: &ViewCtx) -> Option<Expr> {
    use ViewHelperKind::*;
    match kind {
        TurboStreamFrom { streamables } => Some(view_helpers_call(
            "turbo_stream_from",
            vec![view_stream_name(streamables, ctx)?],
        )),
        DomId { record, prefix } => {
            let mut args = vec![(*record).clone()];
            if let Some(p) = prefix {
                args.push((*p).clone());
            }
            Some(view_helpers_call("dom_id", args))
        }
        Pluralize { count, word } => {
            // spinel-blog uses `Inflector.pluralize` for the count-
            // labeling form (separate concern from ActiveSupport's
            // string pluralization helpers).
            Some(inflector_call(
                "pluralize",
                vec![(*count).clone(), (*word).clone()],
            ))
        }
        Truncate { text, opts } => {
            // Spinel-blog convention: `truncate` returns a plain
            // (non-html-safe) string, so its output gets wrapped in
            // `html_escape` before going to `io`. Other helpers
            // (link_to / button_to / dom_id / turbo_stream_from /
            // pluralize / content_for_get) return strings that are
            // already escape-correct and pass through raw.
            // `.to_s` on the text for the same reason as link_to's
            // label: it is routinely a nullable column read
            // (`truncate(article.body, length: 100)`) and the runtime
            // helper is monomorphic `(String, …) -> String`. Rails
            // renders nil as the empty string here too.
            let mut args = vec![lit_str_coerce((*text).clone())];
            if let Some(o) = opts {
                args.push((*o).clone());
            }
            let truncated = view_helpers_call("truncate", args);
            Some(view_helpers_call("html_escape", vec![truncated]))
        }
        ContentForGetter { slot } => Some(view_helpers_call(
            "content_for_get",
            vec![lit_sym(Symbol::from(*slot))],
        )),
        LinkTo { text, url, opts } => emit_link_to_inline(text, url, *opts, ctx),
        ButtonTo { text, target, opts } => emit_button_to_inline(text, target, *opts, ctx),
        // Layout-`<head>` helpers — bare zero-arg ViewHelpers calls.
        CsrfMetaTags => Some(view_helpers_call("csrf_meta_tags", Vec::new())),
        CspMetaTag => Some(view_helpers_call("csp_meta_tag", Vec::new())),
        // `javascript_importmap_tags` consumes per-app importmap data:
        // emit `Importmap.pins` and `Importmap.entry` as args. Both
        // are class methods on the generated `Importmap` module
        // (lower_importmap_to_library_functions). The runtime helper
        // iterates pins to emit modulepreload links + the importmap-
        // script JSON, matching Rails' shape. Other targets (Rust /
        // Python / Crystal / Go) still consume `Importmap::PINS` as
        // a constant; their per-target emit will migrate to the
        // method form when each target retires its lowered_importmap
        // emitter.
        JavascriptImportmapTags => {
            let pins = super::send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Importmap")] },
                )),
                "pins",
                vec![],
                None,
                true,
            );
            let entry = super::send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const { path: vec![Symbol::from("Importmap")] },
                )),
                "entry",
                vec![],
                None,
                true,
            );
            Some(view_helpers_call("javascript_importmap_tags", vec![pins, entry]))
        }
        // `<%= stylesheet_link_tag :app, "data-turbo-track":
        // "reload" %>` — first arg is the stylesheet group. When the
        // arg is the `:app` symbol AND the app has multiple stylesheets
        // ingested from `app/assets/stylesheets/` + `app/assets/builds/`,
        // expand to one call per stylesheet (matching Rails' Propshaft
        // resolution). Otherwise pass through with the symbol→string
        // conversion the runtime expects.
        StylesheetLinkTag { name, opts } => {
            if let ExprNode::Lit { value: Literal::Sym { value } } = &*name.node {
                if value.as_str() == "app" && !ctx.stylesheets.is_empty() {
                    let mut calls: Vec<Expr> = Vec::new();
                    for sheet in &ctx.stylesheets {
                        let mut args = vec![lit_str(sheet.clone())];
                        if let Some(o) = opts {
                            args.push((*o).clone());
                        }
                        calls.push(view_helpers_call("stylesheet_link_tag", args));
                    }
                    // Chain calls with " + \"\\n\" + " so subsequent links
                    // render flush-left — matches Rails' helper output where
                    // only the first stylesheet gets the source indent and
                    // the rest are at column 0. (Same shape as Rails'
                    // `javascript_importmap_tags` modulepreload list.)
                    let sep = lit_str("\n".to_string());
                    let mut chain = calls.remove(0);
                    for call in calls {
                        chain = send(Some(chain), "+", vec![sep.clone()], None, false);
                        chain = send(Some(chain), "+", vec![call], None, false);
                    }
                    return Some(chain);
                }
            }
            let name_expr = match &*name.node {
                ExprNode::Lit { value: Literal::Sym { value } } => {
                    lit_str(value.as_str().to_string())
                }
                _ => (*name).clone(),
            };
            let mut args = vec![name_expr];
            if let Some(o) = opts {
                args.push((*o).clone());
            }
            Some(view_helpers_call("stylesheet_link_tag", args))
        }
        // ContentForSetter is statement-level (handled in walk_stmt);
        // returning None here forwards a TODO so any unexpected
        // `<%= content_for :slot, body %>` form surfaces as a no-op
        // append rather than silent-passing through.
        ContentForSetter { .. } => None,
    }
}

/// Spell the stream name for `turbo_stream_from <streamables…>`. The
/// SUBSCRIBE half of the wire whose PUBLISH half is the model-side
/// `broadcast_*_to` lowering; both call `lower::broadcasts::stream_name`
/// so the two names cannot drift apart.
///
/// A bare name matching a model singular is that record (`room` →
/// `room_#{room.id}`) — the same name signal `apply_route_param_lowering`
/// and the view lowerer's `ivar_ty` already commit to. A literal is its
/// own text. Anything else declines the whole call rather than
/// subscribing to a name we guessed at; a `channel:` kwarg is dropped
/// with a ledger line, since our cable transport has no per-channel
/// classes to route to.
fn view_stream_name(streamables: &[Expr], ctx: &ViewCtx) -> Option<Expr> {
    use crate::lower::broadcasts::Streamable;
    // ONE streamable that is not a bare record name is the app spelling
    // the stream itself — `turbo_stream_from "article_#{@article.id}
    // _comments"` is the blog's, and the name it builds is already the
    // whole convention. Pass it through untouched; only the multi-part
    // form needs a spelling, because only it has parts to join.
    if let [only] = streamables {
        let bare = matches!(&*only.node,
            ExprNode::Lit { value: Literal::Sym { .. } })
            || streamable_record_name(only).is_some_and(|n| ctx.model_singulars.contains(&n));
        if !bare {
            return Some((*only).clone());
        }
    }
    let mut parts = Vec::new();
    for arg in streamables {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => {
                parts.push(Streamable::Literal(value.as_str().to_string()))
            }
            ExprNode::Lit { value: Literal::Str { value } } => {
                parts.push(Streamable::Literal(value.clone()))
            }
            // The trailing options hash (`channel: "RoomMessagesChannel"`).
            ExprNode::Hash { .. } => {
                crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
                    "turbo_stream_from",
                    "channel: option",
                    arg.span,
                    "custom cable channel not modeled",
                    "the subscription is emitted on the default channel; a \
                     per-channel authorization class has no equivalent here"
                        .to_string(),
                ));
            }
            _ => {
                let name = streamable_record_name(arg)?;
                if !ctx.model_singulars.contains(&name) {
                    return None;
                }
                parts.push(Streamable::Record {
                    singular: name,
                    id: send(Some(arg.clone()), "id", vec![], None, false),
                });
            }
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(crate::lower::broadcasts::stream_name(&parts))
}

/// Inline-expand `link_to text, url, opts` into a single
/// StringInterp Expr: `<a href="<escaped_href>"<opts>>
/// <html_escape(text)></a>`. Retires the runtime `ViewHelpers.link_to`
/// call (HashMap-shaped opts) — same architectural rationale as the
/// form_with macro-inline (Wedges 1b-i + 1b-ii). The URL position
/// goes through `emit_url_arg` so path-helpers + record refs
/// resolve to `RouteHelpers.<x>_path(...)` calls before lowering
/// into the interp.
fn emit_link_to_inline(
    text: &Expr,
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let (prefix, suffix) = link_to_wrapper_parts(url, opts, ctx)?;
    let mut parts = prefix;
    parts.push(InterpPart::Expr {
        // Same `.to_s` the bare-interpolation path applies: the label
        // can be a nullable column read (`link_to article.title, …`),
        // and `html_escape` is deliberately monomorphic `(String) ->
        // String`. `nil.to_s == ""` is what Rails renders.
        expr: view_helpers_call("html_escape", vec![lit_str_coerce(text.clone())]),
    });
    parts.extend(suffix);
    Some(string_interp(parts))
}

/// The anchor both spellings share, split where the link's CONTENT
/// goes. Same division of labour as [`button_to_wrapper_parts`].
fn link_to_wrapper_parts(
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<(Vec<InterpPart>, Vec<InterpPart>)> {
    let url_expr = emit_url_arg(url, ctx)?;
    let opts_entries = hash_entries(opts);
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: "<a href=\"".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![url_expr]),
    });
    parts.push(InterpPart::Text {
        value: "\"".to_string(),
    });
    append_attr_parts(&mut parts, &opts_entries);
    parts.push(InterpPart::Text {
        value: ">".to_string(),
    });
    let suffix = vec![InterpPart::Text { value: "</a>".to_string() }];
    Some((parts, suffix))
}

/// Inline-expand `button_to text, url, opts` into the wrapping
/// `<form action="..." method="post" class="<form_class>">...</form>`
/// + method override hidden input + `<button>` + CSRF token hidden
/// input shape Rails' runtime button_to produces. `method:` and
/// `form_class:` are peeled off `opts` at lower time; the rest
/// flow as `<button>` element attributes. CSRF + _method override
/// go through the same runtime primitives form_with uses.
fn emit_button_to_inline(
    text: &Expr,
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let (prefix, suffix) = button_to_wrapper_parts(url, opts, ctx)?;
    let mut parts = prefix;
    parts.push(InterpPart::Expr {
        // Same `.to_s` the bare-interpolation path applies: the label
        // can be a nullable column read (`link_to article.title, …`),
        // and `html_escape` is deliberately monomorphic `(String) ->
        // String`. `nil.to_s == ""` is what Rails renders.
        expr: view_helpers_call("html_escape", vec![lit_str_coerce(text.clone())]),
    });
    parts.extend(suffix);
    Some(string_interp(parts))
}

/// The BLOCK spelling of a helper whose positional form inline-expands
/// — `<%= button_to url, class: "…" do %> …markup… <% end %>`
/// (campfire's join-code regenerate button) and the same for `link_to`
/// (five campfire templates, `messages/_actions` among them).
///
/// Emits STATEMENTS rather than one expression, the shape
/// `emit_tag_builder_inline` uses: the opening markup appends, the block
/// body walks into the SAME accumulator, then the closing appends. That
/// is not a stylistic choice — the block body is template markup
/// (`image_tag`, a literal `<span>`), and Rails treats what the block
/// yields as the element's HTML content. Routing it through the
/// positional form's `html_escape` would render the markup as visible
/// text.
///
/// `None` when the name is not one of those, or when the URL position
/// is a shape `emit_url_arg` declines — the caller then leaves the site
/// to the generic block arm, exactly as before.
pub(super) fn emit_inline_helper_block(
    method: &str,
    args: &[Expr],
    body: &Expr,
    block_params: &[Symbol],
    ctx: &ViewCtx,
) -> Option<Vec<Expr>> {
    // Both helpers put the URL first and an optional attributes hash
    // second; the block replaces the positional form's leading label.
    let url = args.first()?;
    let opts = args.get(1);
    let (prefix, suffix) = match method {
        "button_to" => button_to_wrapper_parts(url, opts, ctx)?,
        "link_to" => link_to_wrapper_parts(url, opts, ctx)?,
        _ => return None,
    };
    let mut out = vec![super::accumulator_append_call(string_interp(prefix), ctx)];
    let inner_ctx = ctx.with_locals(block_params.iter().map(|p| p.as_str().to_string()));
    out.extend(super::walker::walk_body(body, &inner_ctx));
    out.push(super::accumulator_append_call(string_interp(suffix), ctx));
    Some(out)
}

/// The wrapper both spellings share, split where the button's CONTENT
/// goes: everything up to the open `<button …>` and everything from
/// `</button>` on. One owner for the markup, so the two forms cannot
/// drift.
fn button_to_wrapper_parts(
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<(Vec<InterpPart>, Vec<InterpPart>)> {
    let url_expr = emit_url_arg(url, ctx)?;
    let mut opts_entries = hash_entries(opts);
    let method_expr = take_opt(&mut opts_entries, "method").unwrap_or_else(default_method_sym);
    let form_class_expr =
        take_opt(&mut opts_entries, "form_class").unwrap_or_else(default_form_class);
    // Remaining entries become `<button>` attributes.
    let button_opts = opts_entries;

    let mut parts: Vec<InterpPart> = Vec::new();
    // <form action="<href>" method="post" class="<form_class>">
    parts.push(InterpPart::Text {
        value: "<form action=\"".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![url_expr]),
    });
    parts.push(InterpPart::Text {
        value: "\" method=\"post\" class=\"".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![form_class_expr]),
    });
    parts.push(InterpPart::Text {
        value: "\">".to_string(),
    });
    // _method hidden input (empty string when method is :get/:post).
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("method_override_input", vec![method_expr]),
    });
    // <button type="submit" <button_opts>>
    parts.push(InterpPart::Text {
        value: "<button type=\"submit\"".to_string(),
    });
    append_attr_parts(&mut parts, &button_opts);
    parts.push(InterpPart::Text {
        value: ">".to_string(),
    });
    let suffix = vec![
        InterpPart::Text { value: "</button>".to_string() },
        // CSRF authenticity_token hidden input.
        InterpPart::Expr { expr: view_helpers_call("csrf_token_hidden_input", Vec::new()) },
        InterpPart::Text { value: "</form>".to_string() },
    ];
    Some((parts, suffix))
}

/// Extract the entries Vec from a `Hash` literal opts arg, or empty
/// when no opts were passed. Real-fixture call sites always pass
/// literal Hash kwargs; a non-Hash opts arg falls through as empty
/// (the helper renders with no extra attrs).
fn hash_entries(opts: Option<&Expr>) -> Vec<(Expr, Expr)> {
    let Some(o) = opts else {
        return Vec::new();
    };
    let ExprNode::Hash { entries, .. } = &*o.node else {
        return Vec::new();
    };
    entries.clone()
}

/// Translate the URL-position argument (`link_to text, URL, opts`)
/// into spinel shape: literal strings pass through, path-helper calls
/// rewrite to `RouteHelpers.<name>(...)`, bare local records rewrite
/// to `RouteHelpers.<singular>_path(name.id)`. Nested arrays defer
/// to a later slice (form_with's nested-resource fixture forces them).
fn emit_url_arg(url: &Expr, ctx: &ViewCtx) -> Option<Expr> {
    // A CONDITIONAL url (`@root_path ? "/page/#{n}" : {controller: …}`
    // — every lobsters index's pagination link) is two urls, not one.
    // Resolve each branch on its own; the classifier below sees only the
    // whole `If` and gives up on it, which is how the options-hash
    // branch reached the tag renderer as attributes.
    if let ExprNode::If { cond, then_branch, else_branch } = &*url.node {
        let then_url = emit_url_arg(then_branch, ctx)?;
        let else_url = emit_url_arg(else_branch, ctx)?;
        return Some(Expr::new(
            url.span,
            ExprNode::If {
                cond: cond.clone(),
                then_branch: then_url,
                else_branch: else_url,
            },
        ));
    }
    // An INTERPOLATED literal path (`"/page/#{@page - 1}"`) is already
    // the url — there is nothing to resolve. The classifier only knows
    // the plain-`Lit::Str` case, so this shape fell all the way through
    // to the runtime `link_to`, taking its ternary sibling with it.
    if matches!(&*url.node, ExprNode::StringInterp { .. }) {
        return Some(rewrite_helpers_in_expr(url, ctx));
    }
    // `{controller: …, action: …, page: …}` — Rails' url-options hash.
    // Resolves through the generated route-table lookup (see
    // `lower_url_option_helpers`).
    if let Some(resolved) = emit_url_options_hash(url, ctx) {
        return Some(resolved);
    }
    // Association-reader record URL (`link_to text,
    // showing_user.invited_by_user`) — Rails resolves the record
    // polymorphically through its named route. The reader's target
    // model comes from `reference_targets`; the record rides WHOLE
    // into the route helper (not `.id`) so a custom `to_param`
    // (lobsters' User#to_param = username) shapes the segment exactly
    // as Rails does. Without this arm the call fell back to the
    // runtime `link_to`, which interpolated the record as
    // `#<User:0x…>` into href.
    if let ExprNode::Send { recv: Some(_), method, args, block: None, .. } = &*url.node {
        if args.is_empty() {
            if let Some(target) = ctx.reference_targets.get(method.as_str()) {
                return Some(route_helpers_call(
                    &format!("{target}_path"),
                    vec![url.clone()],
                ));
            }
        }
    }
    // Bare `<x>_url` absolute helpers (`button_to "Verify",
    // twofa_verify_url`) — RouteHelpers has no `_url` functions;
    // ground to the shared absolute interp.
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*url.node {
        if let Some(stem) = method.as_str().strip_suffix("_url") {
            return Some(super::absolute_url_interp(stem, args.clone()));
        }
    }
    let is_local = |n: &str| ctx.is_local(n);
    let kind = classify_view_url_arg(url, &is_local)?;
    match kind {
        ViewUrlArg::Literal { value } => Some(lit_str(value.to_string())),
        // The local already holds the url — read it.
        ViewUrlArg::LocalUrl { name } => Some(var_ref(Symbol::from(name))),
        ViewUrlArg::PathHelper { name, args } => {
            let route_args: Vec<Expr> = args.iter().map(|a| rewrite_path_arg(a, ctx)).collect();
            Some(route_helpers_call(name, route_args))
        }
        ViewUrlArg::RecordRef { name } => {
            let singular = singularize(name);
            let id_expr = send(
                Some(var_ref(Symbol::from(name))),
                "id",
                Vec::new(),
                None,
                false,
            );
            Some(route_helpers_call(
                &format!("{singular}_path"),
                vec![id_expr],
            ))
        }
        // `[comment.article, comment]` — nested-resource array. Each
        // element resolves to a (singular_name, id_expr) pair via
        // `classify_nested_url_element`; the path-helper name is the
        // underscore-joined singulars + `_path`, and the args are
        // each element's id expression. So `[comment.article,
        // comment]` → `RouteHelpers.article_comment_path
        // (comment.article_id, comment.id)`. Returns None if any
        // element doesn't classify (literals, complex chains).
        ViewUrlArg::NestedArray { elements } => {
            let is_local = |n: &str| ctx.is_local(n);
            let mut singulars: Vec<String> = Vec::new();
            let mut path_args: Vec<Expr> = Vec::new();
            for el in elements {
                let kind = classify_nested_url_element(el, &is_local)?;
                let (singular, id_expr) = nested_element_parts(&kind);
                singulars.push(singular);
                path_args.push(id_expr);
            }
            let path_name = format!("{}_path", singulars.join("_"));
            Some(route_helpers_call(&path_name, path_args))
        }
    }
}

/// `{controller: controller_name, action: action_name, page: @page + 1}`
/// → `RouteHelpers.path_for_controller_action_page(controller_name,
/// action_name, (@page + 1).to_s)`. The resolver is generated from the
/// app's own route table (`lower_url_option_helpers`), keyed on the same
/// extra-key set, so the two sides agree by construction.
///
/// Every value goes through `.to_s`: the resolver's params are String
/// (a path segment is text) while `page:` arrives as an Integer
/// expression. Returns None for any hash that isn't this form, so an
/// ordinary opts hash in url position still falls through.
fn emit_url_options_hash(url: &Expr, ctx: &ViewCtx) -> Option<Expr> {
    let ExprNode::Hash { entries, .. } = &*url.node else {
        return None;
    };
    let mut by_key: Vec<(String, Expr)> = Vec::new();
    for (k, v) in entries {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node else {
            return None;
        };
        by_key.push((value.as_str().to_string(), v.clone()));
    }
    let take = |name: &str| -> Option<Expr> {
        by_key.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone())
    };
    let controller = take("controller")?;
    let action = take("action")?;
    let mut extras: Vec<(String, Expr)> = by_key
        .iter()
        .filter(|(k, _)| k != "controller" && k != "action")
        .cloned()
        .collect();
    extras.sort_by(|a, b| a.0.cmp(&b.0));
    let extra_names: Vec<String> = extras.iter().map(|(k, _)| k.clone()).collect();
    let to_s = |e: Expr| send(Some(e), "to_s", Vec::new(), None, false);
    let mut args = vec![to_s(controller), to_s(action)];
    args.extend(extras.into_iter().map(|(_, v)| to_s(v)));
    let args = args
        .into_iter()
        .map(|a| rewrite_helpers_in_expr(&a, ctx))
        .collect();
    Some(route_helpers_call(
        &crate::lower::url_options_helper_name(&extra_names),
        args,
    ))
}

/// Each element of a nested URL array resolves to `(singular, id_expr)`.
/// `DirectLocal { name: "comment" }` → `("comment", comment.id)`.
/// `Association { owner: "comment", assoc: "article" }` →
/// `("article", comment.article_id)` — the FK column on the owner is
/// the load-bearing source so we don't have to dereference the
/// belongs_to read just to get the id.
fn nested_element_parts(kind: &NestedUrlElement<'_>) -> (String, Expr) {
    match kind {
        NestedUrlElement::DirectLocal { name } => {
            let id_expr = send(
                Some(var_ref(Symbol::from(*name))),
                "id",
                Vec::new(),
                None,
                false,
            );
            ((*name).to_string(), id_expr)
        }
        NestedUrlElement::Association { owner, assoc } => {
            let fk = format!("{assoc}_id");
            let id_expr = send(
                Some(var_ref(Symbol::from(*owner))),
                &fk,
                Vec::new(),
                None,
                false,
            );
            ((*assoc).to_string(), id_expr)
        }
    }
}

/// `link_to`'s `edit_article_path(article)` argument: the bare local
/// `article` should pass as `article.id`, mirroring how nav links flow
/// through Rails url-for. Accepts both `Var` (the post-ivar-rewrite
/// shape) and the bareword `Send { recv: None, args: [], block: None }`
/// shape Prism produces for partial-scope locals. Anything else
/// passes through unchanged.
fn rewrite_path_arg(arg: &Expr, ctx: &ViewCtx) -> Expr {
    let local_name = match &*arg.node {
        ExprNode::Var { name, .. } if ctx.is_local(name.as_str()) => Some(name.clone()),
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            ..
        } if args.is_empty() && ctx.is_local(method.as_str()) => Some(method.clone()),
        _ => None,
    };
    match local_name {
        Some(name) => send(
            Some(var_ref(name)),
            "id",
            Vec::new(),
            None,
            false,
        ),
        None => arg.clone(),
    }
}

/// The model singular a `turbo_stream_from` streamable names, if it
/// names one.
///
/// `bare_record_name`'s three shapes (local, ivar, bare call) plus
/// `Current.<name>` — campfire's sidebar subscribes with
/// `turbo_stream_from Current.user, :rooms`, and `Current` is the app's
/// own `CurrentAttributes`, so the METHOD is the model singular there
/// the way the variable name is elsewhere. Declining it declined the
/// whole call, and an unlowered `turbo_stream_from` in a view body is a
/// NoMethodError at render — the subscribe half of the wire simply
/// missing while its publish half worked.
///
/// Kept local rather than folded into `bare_record_name`: this asks
/// "what record does this expression denote", which is the streamable
/// question. `bare_record_name`'s other callers ask narrower ones.
fn streamable_record_name(e: &Expr) -> Option<String> {
    if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
        if args.is_empty() {
            if let ExprNode::Const { path } = &*r.node {
                if path.last().map(|s| s.as_str()) == Some("Current") {
                    return Some(method.as_str().to_string());
                }
            }
        }
    }
    bare_record_name(e)
}
