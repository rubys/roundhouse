//! Rails' `tag.<name>(…)` TagBuilder → the HTML string it builds.
//!
//! `tag` is a proxy object whose `method_missing` turns any name into an
//! element (`tag.div`, `tag.details`, `tag.meta`). Nothing in an emitted
//! tree supplies it: a helper lowers to a module function with no view
//! context, so the call survives into the emit as a literal
//! `tag.details(…)` with `tag` unbound and takes the whole page down at
//! render. A runtime proxy is not the fix either — `method_missing` is
//! exactly what [[feedback_runtime_must_be_statically_resolvable]] rules
//! out, and a strict target cannot compile it at all.
//!
//! The element name is known at COMPILE time, though, which is what
//! makes this a lowering rather than a runtime concern. campfire writes
//! ~55 of these across 21 files, so it is the systematic surface
//! standing between the emit and any rendered page.
//!
//! ## What it produces
//!
//! Four shapes, by what the call carries:
//!
//! | source                              | result                                |
//! |-------------------------------------|---------------------------------------|
//! | `tag.meta name: "x"` (void element) | `"<meta name=\"x\">"`                 |
//! | `tag.div class: "x"`                | `"<div class=\"x\"></div>"`           |
//! | `tag.span "text", class: "x"`       | `"<span class=\"x\">text</span>"`     |
//! | `tag.div(class: "x") { … }`         | `"<div class=\"x\">" + capture { … } + "</div>"` |
//!
//! Attributes go through [`attr_parts::append_attr_parts`], the same
//! renderer the form builder uses — so `data:`/`aria:` hashes kebab-case
//! and flatten, boolean attributes render Rails' way (`required` when
//! truthy, ABSENT when falsy), and a nil value drops the attribute
//! instead of rendering `key=""`. Sharing that is the point: the rule is
//! Rails' and there should be one copy of it.
//!
//! The BLOCK form re-uses [`crate::lower::capture_inline`] rather than
//! reimplementing the buffer dance: campfire's blocks are written with
//! `concat`, which is precisely what `capture` models, so this pass
//! synthesizes the `capture { … }` that pass already knows how to
//! flatten into an accumulator. Hence the ordering constraint — this
//! runs BEFORE `capture_inline`, and before `html_safe`, which needs to
//! see the marker described next.
//!
//! ## Why the result is marked `.html_safe`
//!
//! Rails' tag builder returns a SafeBuffer, so `<%= tag.meta … %>` is
//! not escaped. Lowered to a bare string it would be — the view walker
//! escapes every interpolation it cannot prove safe, and `&lt;meta&gt;`
//! is not a meta tag. Wrapping the result in `.html_safe` says so in the
//! language the rest of the pipeline already speaks: the view walker's
//! `html_safe_value` unwraps it, and `html_safe` folds the marker away
//! AND registers the enclosing helper in `app.html_safe_methods`, so
//! callers of `translation_button` don't escape it either.
//!
//! ## Where it declines
//!
//! Attributes that are not a literal Hash — campfire's `tag.time
//! **attributes, datetime: …` desugars to a `merge` chain — cannot be
//! walked at compile time. Those fall back to the runtime
//! `ActionView::ViewHelpers.content_tag`, which is ruby-family only, and
//! are ledgered. The fallback is what makes the pass TOTAL: every
//! `tag.<name>` site becomes something that runs, which is the whole
//! point when the alternative is an unbound `tag`.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::lower::view_to_library::attr_parts::{append_attr_parts, string_interp};
use crate::lower::view_to_library::{lit_str, view_helpers_call};

/// HELPER METHOD BODIES ONLY — views are deliberately not walked.
///
/// ERB already has an owner for this: the view walker recognizes `<%=
/// tag.<el> do %> … <% end %>` and inline-expands it
/// (`form_with::emit_tag_builder_inline`), because an ERB block body is
/// template buffer ops rather than plain Ruby and has to be walked, not
/// captured. Running this pass over views too would race that — it runs
/// first, so the walker's arm would never fire, and the `capture { … }`
/// synthesized here cannot express template ops. One owner per body
/// kind; the walker's own non-block gap is fixed in the walker.
pub fn apply_tag_builder_lowering(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    super::for_each_hook_body(app, &mut |body| rewrite(body, &mut diags));
    diags
}

/// HTML elements that never take content, so they render with no
/// closing tag. Rails' `TagBuilder::VOID_ELEMENTS`, kept verbatim.
fn is_void_element(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "keygen"
            | "link"
            | "meta"
            | "source"
            | "track"
            | "wbr"
    )
}

/// Is this an HTML element name we are willing to build?
///
/// Rails' proxy answers ANY name (`tag.foo_bar` → `<foo-bar>`, its
/// custom-element form), but an open rule would also claim a same-named
/// method on some other receiver. Requiring a known element keeps the
/// match honest; an unknown name declines and is ledgered, which is
/// visible, rather than being silently rewritten.
fn is_html_element(name: &str) -> bool {
    is_void_element(name)
        || matches!(
            name,
            "a" | "abbr"
                | "address"
                | "article"
                | "aside"
                | "audio"
                | "b"
                | "blockquote"
                | "body"
                | "button"
                | "canvas"
                | "caption"
                | "cite"
                | "code"
                | "colgroup"
                | "data"
                | "datalist"
                | "dd"
                | "del"
                | "details"
                | "dfn"
                | "dialog"
                | "div"
                | "dl"
                | "dt"
                | "em"
                | "fieldset"
                | "figcaption"
                | "figure"
                | "footer"
                | "form"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "head"
                | "header"
                | "hgroup"
                | "html"
                | "i"
                | "iframe"
                | "ins"
                | "kbd"
                | "label"
                | "legend"
                | "li"
                | "main"
                | "map"
                | "mark"
                | "menu"
                | "meter"
                | "nav"
                | "noscript"
                | "object"
                | "ol"
                | "optgroup"
                | "option"
                | "output"
                | "p"
                | "picture"
                | "pre"
                | "progress"
                | "q"
                | "rp"
                | "rt"
                | "ruby"
                | "s"
                | "samp"
                | "script"
                | "section"
                | "select"
                | "slot"
                | "small"
                | "span"
                | "strong"
                | "style"
                | "sub"
                | "summary"
                | "sup"
                | "table"
                | "tbody"
                | "td"
                | "template"
                | "textarea"
                | "tfoot"
                | "th"
                | "thead"
                | "time"
                | "title"
                | "tr"
                | "u"
                | "ul"
                | "var"
                | "video"
        )
}

/// The bare `tag` helper — a receiver-less, argument-less call.
///
/// This is the discriminator that keeps the pass off a LOCAL named
/// `tag`: campfire's `Opengraph::Document` iterates parsed HTML nodes as
/// `|tag|` and calls `tag.key?("property")`, which ingest represents as
/// a `Var` receiver, not a call. Prism makes that distinction for us —
/// a bare identifier is a `CallNode` only when no local shadows it.
fn is_tag_helper(recv: &Expr) -> bool {
    matches!(
        &*recv.node,
        ExprNode::Send { recv: None, method, args, block: None, .. }
            if method.as_str() == "tag" && args.is_empty()
    )
}

fn rewrite(expr: &mut Expr, diags: &mut Vec<Diagnostic>) {
    expr.node
        .for_each_child_mut(&mut |child| rewrite(child, diags));

    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*expr.node else {
        return;
    };
    if !is_tag_helper(recv) {
        return;
    }
    let name = method.as_str().to_string();
    if !is_html_element(&name) {
        diags.push(residue(expr, &format!("`{name}` is not a known HTML element")));
        return;
    }

    // Split the arguments Rails' way: a trailing Hash is the attributes,
    // anything before it is the content.
    let (content, opts): (Option<Expr>, Option<Vec<(Expr, Expr)>>) = match args.last() {
        Some(last) => match &*last.node {
            ExprNode::Hash { entries, .. } => {
                (args[..args.len() - 1].first().cloned(), Some(entries.clone()))
            }
            _ => (args.first().cloned(), None),
        },
        None => (None, None),
    };
    // A non-Hash trailing argument that isn't the only argument means the
    // attributes are a computed expression (the `**attrs` merge chain).
    let computed_attrs = args.len() > 1 && opts.is_none();
    let block = block.clone();

    if computed_attrs {
        diags.push(residue(
            expr,
            "attributes are a computed expression, not a literal hash",
        ));
        *expr = content_tag_fallback(&name, content, args.last().cloned(), block, expr.span);
        return;
    }

    let opts = opts.unwrap_or_default();
    let span = expr.span;
    let mut parts: Vec<InterpPart> = vec![InterpPart::Text { value: format!("<{name}") }];
    append_attr_parts(&mut parts, &opts);
    parts.push(InterpPart::Text { value: ">".to_string() });

    if !is_void_element(&name) {
        match (block, content) {
            // `tag.div(…) { … }` — the block IS the content, and it is
            // written with `concat`, which is what `capture` models.
            (Some(block), _) => {
                parts.push(InterpPart::Expr { expr: capture_call(block, span) });
            }
            (None, Some(content)) => {
                parts.push(InterpPart::Expr { expr: escaped_content(content) });
            }
            (None, None) => {}
        }
        parts.push(InterpPart::Text { value: format!("</{name}>") });
    }

    let mut built = string_interp(parts);
    qualify_view_helpers(&mut built);
    *expr = html_safe(built, span);
}

/// Rewrite the bare `ViewHelpers` receiver the shared attribute
/// renderer builds into the fully-qualified `ActionView::ViewHelpers`.
///
/// `view_helpers_call` emits one segment because its own consumer — the
/// VIEW pipeline — resolves it. This pass also writes into HELPER method
/// bodies, which are ordinary library classes, and there a lone
/// `ViewHelpers` resolves against the enclosing module
/// (`TranslationsHelper::ViewHelpers`) and raises NameError. The ruby
/// emitter's `rewrite_helper_calls` only qualifies RECEIVER-LESS calls,
/// so a const receiver never reaches it.
///
/// Two segments are correct in both homes: every check downstream keys
/// off `path.last() == "ViewHelpers"`, and the view emitter already
/// prints the qualified form.
fn qualify_view_helpers(expr: &mut Expr) {
    expr.node.for_each_child_mut(&mut qualify_view_helpers);
    if let ExprNode::Const { path } = &mut *expr.node {
        if path.len() == 1 && path[0].as_str() == "ViewHelpers" {
            *path = vec![Symbol::from("ActionView"), Symbol::from("ViewHelpers")];
        }
    }
}

/// `capture { <block body> }` — left for `capture_inline` to flatten
/// into an accumulator.
fn capture_call(block: Expr, span: crate::span::Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("capture"),
            args: vec![],
            block: Some(block),
            parenthesized: false,
        },
    )
}

/// Content is escaped, as Rails escapes it — unless the source already
/// marked it safe, which is `tag.style(custom_styles.to_s.html_safe, …)`
/// and would otherwise ship escaped CSS. Reading the marker here is why
/// the pass has to run before `html_safe` folds it away.
fn escaped_content(content: Expr) -> Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*content.node {
        if method.as_str() == "html_safe" && args.is_empty() {
            return to_s(inner.clone());
        }
    }
    view_helpers_call("html_escape", vec![to_s(content)])
}

fn to_s(e: Expr) -> Expr {
    let span = e.span;
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(e),
            method: Symbol::from("to_s"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// Mark the built string as safe HTML. See the header — this is what
/// keeps `<%= tag.meta … %>` from being escaped, and what gets the
/// enclosing helper into `app.html_safe_methods`.
fn html_safe(e: Expr, span: crate::span::Span) -> Expr {
    Expr::new(
        span,
        ExprNode::Send {
            recv: Some(e),
            method: Symbol::from("html_safe"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

/// `ActionView::ViewHelpers.content_tag(:name, content, opts)` — the
/// ruby-family runtime path, for attributes this pass cannot walk.
fn content_tag_fallback(
    name: &str,
    content: Option<Expr>,
    opts: Option<Expr>,
    block: Option<Expr>,
    span: crate::span::Span,
) -> Expr {
    let mut sym = Expr::new(
        span,
        ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } },
    );
    sym.ty = Some(crate::ty::Ty::Sym);
    let body = match block {
        Some(block) => capture_call(block, span),
        None => content.unwrap_or_else(|| lit_str(String::new())),
    };
    let mut args = vec![sym, body];
    if let Some(opts) = opts {
        args.push(opts);
    }
    html_safe(view_helpers_call("content_tag", args), span)
}

fn residue(expr: &Expr, reason: &str) -> Diagnostic {
    super::residue_diagnostic(
        "tag_builder",
        "tag-proxy",
        expr.span,
        reason,
        format!(
            "`tag.<name>` not expanded at compile time ({reason}) — \
             left to the runtime content_tag, which strict targets lack"
        ),
    )
}
