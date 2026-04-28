//! View-helper call construction. Translates classified view-helper
//! kinds (`link_to`, `dom_id`, `pluralize`, …) into spinel-shape
//! `ViewHelpers.*` / `RouteHelpers.*` / `Inflector.*` Sends, and
//! handles URL-position argument lowering.

use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::Symbol;
use crate::naming::singularize;
use crate::span::Span;

use crate::lower::view::{
    classify_nested_url_element, classify_view_url_arg, NestedUrlElement, ViewHelperKind,
    ViewUrlArg,
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
        LinkTo { text, url, opts } => emit_link_or_button("link_to", text, url, *opts, ctx),
        ButtonTo { text, target, opts } => {
            emit_link_or_button("button_to", text, target, *opts, ctx)
        }
        // Layout-`<head>` helpers — bare zero-arg ViewHelpers calls.
        CsrfMetaTags => Some(view_helpers_call("csrf_meta_tags", Vec::new())),
        CspMetaTag => Some(view_helpers_call("csp_meta_tag", Vec::new())),
        // `javascript_importmap_tags` consumes per-app importmap data:
        // emit `Importmap::PINS` (the frozen array from `config/importmap.rb`)
        // and the entry name as args. The runtime helper iterates pins
        // to emit modulepreload links + the importmap-script JSON,
        // matching Rails' shape. Mirrors how the Rust target threads
        // `crate::importmap::PINS` into its helper call.
        JavascriptImportmapTags => {
            let pins = Expr::new(
                Span::synthetic(),
                ExprNode::Const {
                    path: vec![Symbol::from("Importmap"), Symbol::from("PINS")],
                },
            );
            let entry = lit_str("application".to_string());
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
                    // Chain calls with " + \"\\n    \" + " so the rendered
                    // strings concatenate at runtime, matching Rails'
                    // newline-separated multi-link emit. Single-call case
                    // would fall to the else-branch below.
                    let sep = lit_str("\n    ".to_string());
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

fn emit_link_or_button(
    helper: &str,
    text: &Expr,
    url: &Expr,
    opts: Option<&Expr>,
    ctx: &ViewCtx,
) -> Option<Expr> {
    let url_expr = emit_url_arg(url, ctx)?;
    let mut args = vec![text.clone(), url_expr];
    if let Some(o) = opts {
        args.push(o.clone());
    }
    Some(view_helpers_call(helper, args))
}

/// Translate the URL-position argument (`link_to text, URL, opts`)
/// into spinel shape: literal strings pass through, path-helper calls
/// rewrite to `RouteHelpers.<name>(...)`, bare local records rewrite
/// to `RouteHelpers.<singular>_path(name.id)`. Nested arrays defer
/// to a later slice (form_with's nested-resource fixture forces them).
fn emit_url_arg(url: &Expr, ctx: &ViewCtx) -> Option<Expr> {
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
