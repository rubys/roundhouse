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
    append_attr_parts, default_form_class, default_method_sym, string_interp, take_opt,
};
use super::{
    inflector_call, lit_str, lit_sym, route_helpers_call, send, var_ref, view_helpers_call,
    ViewCtx,
};

pub(super) fn emit_view_helper_call(kind: &ViewHelperKind<'_>, ctx: &ViewCtx) -> Option<Expr> {
    use ViewHelperKind::*;
    match kind {
        TurboStreamFrom { channel } => Some(view_helpers_call(
            "turbo_stream_from",
            vec![(*channel).clone()],
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
            let mut args = vec![(*text).clone()];
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
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![text.clone()]),
    });
    parts.push(InterpPart::Text {
        value: "</a>".to_string(),
    });
    Some(string_interp(parts))
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
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![text.clone()]),
    });
    parts.push(InterpPart::Text {
        value: "</button>".to_string(),
    });
    // CSRF authenticity_token hidden input.
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("csrf_token_hidden_input", Vec::new()),
    });
    parts.push(InterpPart::Text {
        value: "</form>".to_string(),
    });
    Some(string_interp(parts))
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
