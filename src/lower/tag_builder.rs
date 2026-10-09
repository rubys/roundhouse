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
//! `ActionView::ViewHelpers.content_tag`, and are ledgered. The fallback
//! is what makes the pass TOTAL: every `tag.<name>` site becomes
//! something that runs, which is the whole point when the alternative is
//! an unbound `tag`.
//!
//! That example was aspirational until 2026-08-31: the fallback's guard
//! required `args.len() > 1`, and a call whose arguments are ENTIRELY
//! keywords has exactly one — so the lone merge chain was read as the
//! tag's CONTENT and stringified into its body. Every `<time>` campfire
//! rendered came out as `<time>{datetime: "…", data: {…}}</time>`. See
//! `is_kwsplat_merge_chain`.
//!
//! `content_tag` is not ruby-family only, despite what this note used to
//! claim: it and `render_attrs` are in the emitted spinel tree too
//! (`runtime/action_view/view_helpers.rb`, with `.rbs` signatures), which
//! is what makes the fallback a real answer on a strict target rather
//! than a deferred failure.
//!
//! ## Also here: `turbo_frame_tag`
//!
//! turbo-rails' frame helper is `tag.turbo_frame(…)` with its arguments
//! rearranged, and it reaches an emitted tree the same way — as an
//! unbound bare call in a helper body. It expands here for the same
//! reasons, through [`crate::lower::view_to_library::turbo_frames`],
//! which owns the rearranging and is shared with the view walker.

use crate::analyze::ClassInfo;
use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::{ClassId, Symbol};
use crate::lower::view_to_library::attr_parts::{append_attr_parts, string_interp};
use crate::lower::view_to_library::turbo_frames;
use crate::lower::view_to_library::{lit_str, view_helpers_call};
use std::collections::{HashMap, HashSet};

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
pub fn apply_tag_builder_lowering(
    app: &mut App,
    registry: &HashMap<ClassId, ClassInfo>,
) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    // For `turbo_frame_tag`'s record test — the same snake-singular set
    // the view lowering's `ViewCtx` carries, built here because a hook
    // body has no view context.
    let models: std::collections::HashSet<String> = app
        .models
        .iter()
        .map(|m| crate::naming::snake_case(m.name.0.as_str()))
        .collect();
    // Named routes the app actually declares — the guard on resolving a
    // polymorphic `link_to [ :edit, @room ]`. Same set
    // `route_format_suffix` and `route_url_options` key off, so the three
    // passes cannot disagree about what a route helper is.
    let helpers = super::route_format_suffix::route_helper_names(app);
    // STI base → subclass dom stems (`room` → `["rooms_open", …]`),
    // from the same `sti_subclass_names` stamp `dom_prefix` dispatches
    // on (`sti_scope` has run by this point in the pipeline). A
    // polymorphic URL names its route through the record's CLASS, and
    // for an STI base the class is a runtime question.
    let sti_stems: std::collections::HashMap<String, Vec<String>> = app
        .models
        .iter()
        .filter(|m| !m.sti_subclass_names.is_empty())
        .map(|m| {
            let stems = m
                .sti_subclass_names
                .iter()
                .map(|sub| {
                    sub.0
                        .as_str()
                        .split("::")
                        .map(crate::naming::snake_case)
                        .collect::<Vec<_>>()
                        .join("_")
                })
                .collect();
            (crate::naming::snake_case(m.name.0.as_str()), stems)
        })
        .collect();
    // A receiver-less `tag` is the builder only where nothing else
    // answers it. A class with its own `tag` — `belongs_to :tag`, a
    // `tag` column, `attr_reader :tag`, a `def tag` — calls ITS reader
    // (`tag.title`), and rewriting that compiled to `"<title></title>"`.
    let owns_tag = classes_owning_tag(app, registry);
    super::for_each_owned_hook_body(app, &mut |owner, body| {
        let own_tag = owner.is_some_and(|c| owns_tag.contains(c));
        rewrite(body, own_tag, &models, &helpers, &sti_stems, &mut diags)
    });
    // Propagate SafeBuffer provenance through helper wrappers before
    // reconciling signatures. Callers such as `link_back` return another
    // HTML-safe helper's result directly; leaving those callers typed as
    // JSON values both breaks Rust and causes views to escape the markup.
    let mut safe_methods = app.html_safe_methods.clone();
    for model in &app.models {
        for item in &model.body {
            if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                if returns_html_safe(&method.body, &safe_methods) {
                    safe_methods.insert(method.name.clone());
                }
            }
        }
    }
    for class in &app.library_classes {
        for method in &class.methods {
            if returns_html_safe(&method.body, &safe_methods) {
                safe_methods.insert(method.name.clone());
            }
        }
    }
    loop {
        let mut newly_safe = Vec::new();
        for model in &app.models {
            for item in &model.body {
                if let crate::dialect::ModelBodyItem::Method { method, .. } = item {
                    if !safe_methods.contains(&method.name)
                        && returns_html_safe(&method.body, &safe_methods)
                    {
                        newly_safe.push(method.name.clone());
                    }
                }
            }
        }
        for class in &app.library_classes {
            for method in &class.methods {
                if !safe_methods.contains(&method.name)
                    && returns_html_safe(&method.body, &safe_methods)
                {
                    newly_safe.push(method.name.clone());
                }
            }
        }
        if newly_safe.is_empty() {
            break;
        }
        safe_methods.extend(newly_safe);
    }
    // A helper that returns a SafeBuffer-marked value has a String
    // result, regardless of the stale declared return type inferred
    // from the pre-lowered `tag.*` call. Keep the safety marker in the
    // body for `html_safe` lowering; only reconcile the typed contract
    // here so Rust's signature agrees with the rewritten expression.
    app.html_safe_methods.extend(safe_methods.iter().cloned());
    for model in &mut app.models {
        for method in model.methods_mut() {
            reconcile_html_result_signature(method, &safe_methods);
        }
    }
    for class in &mut app.library_classes {
        for method in &mut class.methods {
            reconcile_html_result_signature(method, &safe_methods);
        }
    }
    diags
}

fn reconcile_html_result_signature(
    method: &mut crate::dialect::MethodDef,
    safe_methods: &std::collections::BTreeSet<Symbol>,
) {
    if !safe_methods.contains(&method.name) {
        return;
    }
    let declared_result = method
        .signature
        .as_ref()
        .and_then(|signature| match signature {
            crate::ty::Ty::Fn { ret, .. } => Some(ret.as_ref()),
            _ => None,
        })
        .or(method.body.ty.as_ref());
    let result = declared_result.map_or(crate::ty::Ty::Str, safe_html_result_type);
    set_html_result_type(&mut method.body);
    if let Some(crate::ty::Ty::Fn { ret, .. }) = &mut method.signature {
        **ret = result;
    }
}

fn safe_html_result_type(ty: &crate::ty::Ty) -> crate::ty::Ty {
    use crate::ty::Ty;

    match ty {
        Ty::Nil => Ty::Nil,
        Ty::Union { variants } => {
            let mut result = Vec::new();
            for variant in variants {
                let mapped = safe_html_result_type(variant);
                if !result.contains(&mapped) {
                    result.push(mapped);
                }
            }
            match result.as_slice() {
                [only] => only.clone(),
                _ => Ty::Union { variants: result },
            }
        }
        _ => Ty::Str,
    }
}

fn set_html_result_type(expr: &mut Expr) {
    let result = expr
        .ty
        .as_ref()
        .map_or(crate::ty::Ty::Str, safe_html_result_type);
    match &mut *expr.node {
        ExprNode::Seq { exprs } => {
            if let Some(tail) = exprs.last_mut() {
                set_html_result_type(tail);
            }
        }
        ExprNode::Let { body, .. } => set_html_result_type(body),
        ExprNode::If { then_branch, else_branch, .. } => {
            set_html_result_type(then_branch);
            set_html_result_type(else_branch);
        }
        ExprNode::Case { arms, .. } => {
            for arm in arms {
                set_html_result_type(&mut arm.body);
            }
        }
        ExprNode::Lit { value: Literal::Nil } => {}
        _ => expr.ty = Some(crate::ty::Ty::Str),
    }
    expr.ty = Some(result);
}

fn returns_html_safe(body: &Expr, safe_methods: &std::collections::BTreeSet<Symbol>) -> bool {
    match &*body.node {
        ExprNode::Lit { value: Literal::Nil } => true,
        ExprNode::Send { recv, method, args, block, .. } => {
            (recv.is_some() && method.as_str() == "html_safe" && args.is_empty() && block.is_none())
                || safe_methods.contains(method)
        }
        ExprNode::Seq { exprs } => exprs.last().is_some_and(|expr| returns_html_safe(expr, safe_methods)),
        ExprNode::Let { body, .. } => returns_html_safe(body, safe_methods),
        ExprNode::If { then_branch, else_branch, .. } => {
            returns_html_safe(then_branch, safe_methods)
                && returns_html_safe(else_branch, safe_methods)
        }
        ExprNode::Case { arms, .. } => {
            arms.iter().all(|arm| returns_html_safe(&arm.body, safe_methods))
                && arms.iter().any(|arm| matches!(&arm.pattern, crate::expr::Pattern::Wildcard))
        }
        _ => false,
    }
}

/// Classes whose bodies read a bare `tag` as their own method: APP
/// classes with a `tag` instance method — the registry's
/// `instance_methods` already hold associations, columns and `attr_*`
/// readers alongside `def`s — directly or through an app module they
/// include or an app class they inherit from, plus the MODULES such a
/// class includes. A concern's body runs as its includer, so
/// `tag.title` in `Taggable` is `Sticker#tag` once `Sticker` includes
/// it; one includer owning `tag` is enough, because the same body
/// cannot be both that reader and the builder.
///
/// Only app classes count. The framework's own `tag` is the builder
/// (`ActionView::Base#tag` is in the registry), and
/// `include ActionView::Helpers::TagHelper` is dropped by the analyzer
/// as a view-helper marker, so a model that really uses the builder
/// keeps the rewrite.
fn classes_owning_tag(app: &App, registry: &HashMap<ClassId, ClassInfo>) -> HashSet<ClassId> {
    let app_classes: HashSet<&ClassId> = app
        .models
        .iter()
        .map(|m| &m.name)
        .chain(app.library_classes.iter().map(|lc| &lc.name))
        .chain(app.controllers.iter().map(|c| &c.name))
        .collect();
    let tag = Symbol::from("tag");
    let has_tag = |class: &ClassId| {
        // Self, then app ancestors (includes and parents), each once.
        let mut stack = vec![class];
        let mut seen = HashSet::new();
        while let Some(c) = stack.pop() {
            if !app_classes.contains(c) || !seen.insert(c) {
                continue;
            }
            let Some(info) = registry.get(c) else { continue };
            if info.instance_methods.contains_key(&tag) {
                return true;
            }
            stack.extend(info.includes.iter().chain(info.parent.iter()));
        }
        false
    };
    let owners: Vec<&ClassId> = app_classes.iter().copied().filter(|c| has_tag(c)).collect();
    owners
        .iter()
        .flat_map(|c| registry.get(*c).into_iter().flat_map(|info| info.includes.iter()))
        .chain(owners.iter().copied())
        .cloned()
        .collect()
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

/// A CUSTOM element's method name: `tag.lexxy_prompt` builds
/// `<lexxy-prompt>`, Rails' proxy turning each `_` into the `-` a custom
/// element's name must contain (the HTML spec requires one). That hyphen
/// is what makes the rule closed where "any name" would not be: a name
/// with no `_` is not a custom element, so `tag.foo` still declines.
/// campfire's `mention_prompt_tag` (the Lexxy merge) is the first.
fn is_custom_element(name: &str) -> bool {
    name.contains('_')
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
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

/// `own_tag`: the body's class answers `tag` itself, so neither `tag.x`
/// nor `tag(:x)` is the builder there (see [`classes_owning_tag`]). The
/// other helpers this pass expands have names of their own and still
/// apply.
fn rewrite(
    expr: &mut Expr,
    own_tag: bool,
    models: &std::collections::HashSet<String>,
    helpers: &std::collections::HashSet<String>,
    sti_stems: &std::collections::HashMap<String, Vec<String>>,
    diags: &mut Vec<Diagnostic>,
) {
    expr.node.for_each_child_mut(&mut |child| {
        rewrite(child, own_tag, models, helpers, sti_stems, diags)
    });

    if rewrite_turbo_frame_tag(expr, models) {
        return;
    }
    if !own_tag && rewrite_legacy_tag(expr, diags) {
        return;
    }
    if rewrite_button_tag(expr) {
        return;
    }
    if rewrite_button_to_block(expr) {
        return;
    }
    if rewrite_link_to_block(expr, models, helpers, sti_stems, diags) {
        return;
    }

    let ExprNode::Send { recv: Some(recv), method, args, block, .. } = &*expr.node else {
        return;
    };
    if own_tag || !is_tag_helper(recv) {
        return;
    }
    let name = match method.as_str() {
        n if is_html_element(n) => n.to_string(),
        n if is_custom_element(n) => n.replace('_', "-"),
        n => {
            diags.push(residue(expr, &format!("`{n}` is not a known HTML element")));
            return;
        }
    };

    // Split the arguments Rails' way: a trailing Hash is the attributes,
    // anything before it is the content.
    let (mut content, opts): (Option<Expr>, Option<Vec<(Expr, Expr)>>) = match args.last() {
        Some(last) => match &*last.node {
            ExprNode::Hash { entries, .. } => {
                (args[..args.len() - 1].first().cloned(), Some(entries.clone()))
            }
            _ => (args.first().cloned(), None),
        },
        None => (None, None),
    };

    // A LONE computed hash is ATTRIBUTES, and the split above cannot see
    // it. `ingest::expr` desugars a double splat into the `merge` chain it
    // is defined to be, and says so: "A merge chain is a `Send`, not a
    // `Hash`, so it loses the `kwargs` flag and renders as a positional
    // hash argument." The arm above therefore reads the sole argument as
    // the tag's CONTENT and stringifies the attributes into its body:
    //
    //     tag.time **attributes, datetime: …, data: { local_time_target: style }
    //  => <time>{datetime: "2026-08-31T00:38:06Z", data: {local_time_target: :date}}</time>
    //
    // That is what EVERY `<time>` campfire renders looked like — the day
    // separator on every message and every permalink timestamp — behind a
    // 200 and a green suite, because no gate reads the attributes of a tag.
    // `computed_attrs` did not catch it: it required `args.len() > 1`, and
    // a call whose arguments are entirely keywords has exactly one.
    let lone_computed_attrs =
        args.len() == 1 && opts.is_none() && is_kwsplat_merge_chain(&args[0]);
    if lone_computed_attrs {
        content = None;
    }

    // A non-Hash trailing argument that isn't the only argument means the
    // attributes are a computed expression (the `**attrs` merge chain).
    let computed_attrs = opts.is_none() && (args.len() > 1 || lone_computed_attrs);
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

/// turbo-rails' `turbo_frame_tag` written in a HELPER body — the same
/// `<turbo-frame>` element the view walker builds for the ERB spelling,
/// through the same `turbo_frames` expansion, differing only in what
/// goes between the tags: a Ruby block is CAPTURED here, where a
/// template body is walked there.
///
/// campfire's `users/sidebar_helper.rb` is why this half exists — the
/// room page opens with `content_for :sidebar,
/// sidebar_turbo_frame_tag(src: user_sidebar_path)`, and the sidebar's
/// own template calls the same helper with a block.
///
/// Returns whether the call was claimed. A declined argument shape
/// (`frame_open_parts` → None) is left alone, where the emitted call to
/// an undefined `turbo_frame_tag` is loud, rather than being given an
/// invented id.
/// `button_tag [content][, opts] [do … end]` written in a HELPER body.
///
/// The blockless spelling is already inlined for VIEWS
/// (`form_builder::emit_button_tag`); a helper body has no view context,
/// so the call survived into the emit as a literal `button_tag` with
/// nothing to resolve it — campfire's `submit_room_button_tag` is that
/// call, and it stands between the room forms and any rendered page.
///
/// The open tag comes from `form_builder::button_open_parts`, the same
/// builder the view spelling uses, so the two cannot disagree about
/// Rails' `name="button"` / `type="submit"` defaults. Only the CONTENT
/// differs by call site: a Ruby block is CAPTURED here where a template
/// body is walked there — the same split as `turbo_frame_tag`'s two
/// halves above.
///
/// Rails reads a Hash first argument as the OPTIONS when a block is
/// given (`button_tag class: "x" do … end`), which is the campfire
/// spelling and why the content/opts split can't just take the last
/// argument.
fn rewrite_button_tag(expr: &mut Expr) -> bool {
    let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "button_tag" {
        return false;
    }
    let trailing_hash = |e: &Expr| match &*e.node {
        ExprNode::Hash { entries, .. } => Some(entries.clone()),
        _ => None,
    };
    let (content, opts): (Option<Expr>, Vec<(Expr, Expr)>) = match &args[..] {
        [] => (None, Vec::new()),
        [only] => match trailing_hash(only) {
            // A lone Hash is the options — with a block (Rails' rule) and
            // without one (`button_tag class: "x"`, a label-less button).
            Some(entries) => (None, entries),
            None => (Some(only.clone()), Vec::new()),
        },
        [first, rest @ ..] => match rest.last().and_then(trailing_hash) {
            Some(entries) => (Some(first.clone()), entries),
            // `button_tag(a, b)` with a non-Hash tail is not a shape
            // Rails defines; leave it for the emit to be loud about.
            None => return false,
        },
    };
    let span = expr.span;
    let block = block.clone();
    let mut parts = crate::lower::view_to_library::form_builder::button_open_parts(&opts);
    match (block, content) {
        (Some(block), _) => {
            parts.push(InterpPart::Expr { expr: capture_call(block, span) });
        }
        (None, Some(content)) => {
            parts.push(InterpPart::Expr { expr: escaped_content(content) });
        }
        (None, None) => {}
    }
    parts.push(InterpPart::Text { value: "</button>".to_string() });
    let mut built = string_interp(parts);
    qualify_view_helpers(&mut built);
    *expr = html_safe(built, span);
    true
}

/// `button_to url, opts do … end` written in a HELPER body — Rails'
/// BLOCK spelling, where the first argument is the URL and the block
/// supplies the button's content.
///
/// The view walker already expands this for the ERB spelling
/// (`helpers::emit_inline_helper_block`); a helper module never
/// reaches it, so campfire's three `button_to_*` helpers emitted a
/// bare `button_to` no module defines. The runtime's `button_to` is no
/// answer either: it is `(text, href, opts)`, the POSITIONAL form, so
/// qualifying the call would bind the url to `text` and the options to
/// `href` — a shape that compiles and renders nonsense.
///
/// Markup comes from [`button_to_wrapper_markup`], the same owner the
/// walker calls, so the two spellings cannot drift.
///
/// Declines — leaving the site loud rather than guessing — when there
/// is no URL argument or when the options are not a literal Hash (a
/// `merge` chain has nothing to split `method:` and `form_class:` out
/// of at compile time, and those two decide the form, not the button).
fn rewrite_button_to_block(expr: &mut Expr) -> bool {
    let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "button_to" {
        return false;
    }
    let Some(url) = args.first() else { return false };
    let opts = match args.get(1).map(|a| &*a.node) {
        None => Vec::new(),
        Some(ExprNode::Hash { entries, .. }) => entries.clone(),
        Some(_) => return false,
    };
    let span = expr.span;
    let block = block.clone();
    let (mut parts, suffix) =
        crate::lower::view_to_library::helpers::button_to_wrapper_markup(url.clone(), opts);
    // Rails treats what the block yields as the button's HTML content,
    // so it is CAPTURED, not escaped — campfire's blocks are an
    // `image_tag` plus a `<span>`, already markup.
    parts.push(InterpPart::Expr { expr: capture_call(block, span) });
    parts.extend(suffix);
    let mut built = string_interp(parts);
    qualify_view_helpers(&mut built);
    *expr = html_safe(built, span);
    true
}

/// `[ :edit, @room ]` → `edit_room_path(@room)`, Rails' polymorphic URL
/// resolved the only way a static target can resolve it: at transpile
/// time, from the record's model.
///
/// The LAST element is the record and everything before it is a prefix
/// (`[ :edit, @room ]`, `[ :new, :message ]`); Rails builds the helper
/// name by joining them, and so does this. The record's model comes from
/// `bare_record_name`, the same syntactic reading `turbo_frame_tag` and
/// `dom_id` use — which works here because this pass runs before the
/// ivar rewrite, so `@room` is still an `Ivar` and not yet
/// `Current.controller.room`.
///
/// TWO GUARDS, and both matter. The name has to be a model the app
/// actually has, and the assembled helper has to be a NAMED ROUTE the
/// app actually declares — `route_helper_names` is the same set
/// `route_format_suffix` and `route_url_options` key off. Without the
/// second, `[ :edit, @room ]` in an app with no `edit` member route
/// would emit a call to a helper nothing defines: a NameError on CRuby
/// and an unresolved-call build wall on a strict target, which is worse
/// than the ledgered array it replaces.
///
/// A bare call, not `RouteHelpers.<x>` — `route_helper_receiver::
/// qualify_lcs` runs at emit and qualifies it, and the same pass turns
/// the record argument into its id. Synthesizing the qualified form here
/// would be a second, independently-maintained copy of that rule.
fn polymorphic_route_call(
    url: &Expr,
    models: &std::collections::HashSet<String>,
    helpers: &std::collections::HashSet<String>,
    sti_stems: &std::collections::HashMap<String, Vec<String>>,
) -> Option<Expr> {
    let ExprNode::Array { elements, .. } = &*url.node else {
        return None;
    };
    let (record, prefixes) = elements.split_last()?;
    let singular = crate::lower::view_to_library::bare_record_name(record)?;
    if !models.contains(&singular) {
        return None;
    }
    let mut joined_prefix = String::new();
    for prefix in prefixes {
        let ExprNode::Lit { value: Literal::Sym { value } } = &*prefix.node else {
            return None;
        };
        joined_prefix.push_str(value.as_str());
        joined_prefix.push('_');
    }
    let name = format!("{joined_prefix}{singular}_path");
    if !helpers.contains(&name) {
        return None;
    }
    let route_call = |helper: &str| {
        Expr::new(
            url.span,
            ExprNode::Send {
                recv: None,
                method: Symbol::from(helper),
                args: vec![record.clone()],
                block: None,
                parenthesized: true,
            },
        )
    };
    // An STI base's rows belong to subclasses, and Rails'
    // `polymorphic_url` names the route from the record's CLASS: room 1
    // is a `Rooms::Open`, so `[ :edit, @room ]` is
    // `edit_rooms_open_path` (`/rooms/opens/1/edit`), not
    // `edit_room_path`. Hydration here is base-classed, so the call
    // dispatches on `dom_prefix()` — the type-column dispatch
    // `sti_scope`'s stamp synthesizes, whose answer IS the route stem
    // (both are the underscored class name). A subclass whose helper
    // the app does not declare folds into the base arm, same posture as
    // the base-helper guard above; a base with no subclasses keeps the
    // plain call. A nested `If` chain rather than a `Case`, because
    // this lands in app helper code every target compiles and the
    // weakest emitter (TypeScript) has no `case` arm — `if` in value
    // position is the proven shape (the attribute guards ride it).
    let routable: Vec<&String> = sti_stems
        .get(&singular)
        .map(|stems| {
            stems
                .iter()
                .filter(|stem| helpers.contains(&format!("{joined_prefix}{stem}_path")))
                .collect()
        })
        .unwrap_or_default();
    let mut dispatch = route_call(&name);
    for stem in routable.iter().rev() {
        let dom_prefix = Expr::new(
            url.span,
            ExprNode::Send {
                recv: Some(record.clone()),
                method: Symbol::from("dom_prefix"),
                args: Vec::new(),
                block: None,
                parenthesized: true,
            },
        );
        let cond = Expr::new(
            url.span,
            ExprNode::Send {
                recv: Some(dom_prefix),
                method: Symbol::from("=="),
                args: vec![Expr::new(
                    url.span,
                    ExprNode::Lit { value: Literal::Str { value: (*stem).clone() } },
                )],
                block: None,
                parenthesized: false,
            },
        );
        dispatch = Expr::new(
            url.span,
            ExprNode::If {
                cond,
                then_branch: route_call(&format!("{joined_prefix}{stem}_path")),
                else_branch: dispatch,
            },
        );
    }
    Some(dispatch)
}

/// `link_to url, opts do … end` written in a HELPER body — Rails'
/// BLOCK spelling, where the first argument is the URL and the block
/// supplies the anchor's content.
///
/// Exactly the hole `rewrite_button_to_block` above fills, in the
/// helper that is written far more often. The runtime's `link_to` is
/// `(text, href, opts)`, the POSITIONAL form, so qualifying the call
/// binds the URL to `text` and the option hash to `href` — and
/// `render_attrs` then flattens that hash the way it flattens `data:`,
/// so campfire's avatars rendered `<a href-title="User 1"
/// href-class="btn avatar">/users/1</a>`: every attribute prefixed
/// `href-`, the URL as the link TEXT, and the block — an `image_tag` —
/// gone. Forty-two of them on one room page, and it compiled.
///
/// Markup comes from [`link_to_wrapper_markup`], the same owner the
/// view walker calls.
///
/// Options that are not a literal Hash — campfire's `link_to_room`
/// forwards `**attributes` merged with its own `data:`, which arrives
/// as a `merge` chain — render through the runtime's `render_attrs` at
/// run time, the same deferral the view lowering makes for a computed
/// `data:`. Before that arm existed the site was neither rewritten nor
/// ledgered: the qualified positional call bound the option hash to
/// `href` and every sidebar room link rendered as
/// `<a href="{…inspect…}">/rooms/1</a>` — and no test read the anchor.
///
/// Declines when there is no URL argument, leaving the site loud
/// rather than guessed at.
fn rewrite_link_to_block(
    expr: &mut Expr,
    models: &std::collections::HashSet<String>,
    helpers: &std::collections::HashSet<String>,
    sti_stems: &std::collections::HashMap<String, Vec<String>>,
    diags: &mut Vec<Diagnostic>,
) -> bool {
    let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "link_to" {
        return false;
    }
    let Some(url) = args.first() else { return false };
    // A POLYMORPHIC url (`link_to [ :edit, @room ]`) names its route
    // through the record's model, which is a transpile-time question —
    // `ViewHelpers.polymorphic_url` raises at runtime on purpose, because
    // a class-to-route registry is exactly the dynamic dispatch the
    // strict targets cannot carry. Resolve it here, or decline loudly:
    // interpolated as-is the array reaches `html_escape` as an Array and
    // renders the record's `inspect` into the page.
    let url = match &*url.node {
        ExprNode::Array { .. } => match polymorphic_route_call(url, models, helpers, sti_stems) {
            Some(call) => call,
            None => {
                diags.push(super::residue_diagnostic(
                    "tag_builder",
                    "polymorphic-link-url",
                    expr.span,
                    "`link_to [ … ]` in a helper: polymorphic URL",
                    "a polymorphic URL array names its route through the record's \
                     model — this one resolves to no named route helper, so the \
                     array reaches the emitted anchor as itself and renders the \
                     record's `inspect` into the href"
                        .to_string(),
                ));
                return false;
            }
        },
        _ => url.clone(),
    };
    // A Lambda is a block written HERE; a Var is the caller's block
    // FORWARDED (`link_to url, opts, &`, which ingest binds as `__blk`)
    // — campfire's `link_to_room` and `link_to_edit_room` both write the
    // second form. `capture_call` already handles both, guarding the
    // forwarded one against being absent at run time, so declining it
    // here only kept the site broken.
    if !matches!(&*block.node, ExprNode::Lambda { .. } | ExprNode::Var { .. }) {
        return false;
    }
    let span = expr.span;
    let block = block.clone();
    let (mut parts, suffix) = match args.get(1).map(|a| &*a.node) {
        None => crate::lower::view_to_library::helpers::link_to_wrapper_markup(url, Vec::new()),
        Some(ExprNode::Hash { entries, .. }) => {
            crate::lower::view_to_library::helpers::link_to_wrapper_markup(url, entries.clone())
        }
        // Computed options: the runtime renders them, in Rails' order
        // (options first, `href` last — the same order the literal arm
        // writes).
        Some(_) => {
            let opts = args[1].clone();
            let parts = vec![
                InterpPart::Text { value: "<a".to_string() },
                InterpPart::Expr { expr: view_helpers_call("render_attrs", vec![opts]) },
                InterpPart::Text { value: " href=\"".to_string() },
                InterpPart::Expr { expr: view_helpers_call("html_escape", vec![url]) },
                InterpPart::Text { value: "\">".to_string() },
            ];
            (parts, vec![InterpPart::Text { value: "</a>".to_string() }])
        }
    };
    // Rails treats what the block yields as the anchor's HTML content,
    // so it is CAPTURED, not escaped — the same rule the `button_to`
    // twin follows, and campfire's blocks are an `image_tag` plus a
    // `<span>`, already markup.
    parts.push(InterpPart::Expr { expr: capture_call(block, span) });
    parts.extend(suffix);
    let mut built = string_interp(parts);
    qualify_view_helpers(&mut built);
    *expr = html_safe(built, span);
    true
}

/// The LEGACY function form — `tag(:meta, name: "x", content: "y")`,
/// which campfire's `current_user_meta_tags` writes twice and every
/// page's `<head>` therefore depends on.
///
/// Same proxy object, different call: `tag` with arguments is
/// ActionView's original `tag(name, options)` rather than the
/// `method_missing` builder, and it renders DIFFERENTLY. MEASURED
/// against Rails 8:
///
///   tag(:meta, name: "n", content: "c")  =>  <meta name="n" content="c" />
///   tag.meta(name: "n")                  =>  <meta name="n">
///
/// The legacy form keeps the XHTML-style close, the builder does not.
/// Routing this through the builder's renderer would have been
/// DOM-equivalent and byte-wrong; the two spellings share
/// `append_attr_parts` and nothing else.
///
/// Void-ness does not enter into it: the legacy form self-closes
/// whatever it is handed, which is why it takes no block and no
/// content argument here.
fn rewrite_legacy_tag(expr: &mut Expr, diags: &mut Vec<Diagnostic>) -> bool {
    let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "tag" || args.is_empty() {
        return false;
    }
    let name = match &*args[0].node {
        ExprNode::Lit { value: Literal::Sym { value } } => value.as_str().to_string(),
        ExprNode::Lit { value: Literal::Str { value } } => value.clone(),
        // A computed element name has nothing to expand at compile
        // time; leave it for the ledger rather than guess.
        _ => return false,
    };
    if !is_html_element(&name) {
        diags.push(residue(expr, &format!("`{name}` is not a known HTML element")));
        return false;
    }
    let opts = match args.get(1).map(|a| &*a.node) {
        None => Vec::new(),
        Some(ExprNode::Hash { entries, .. }) => entries.clone(),
        // `tag(:meta, attrs)` over a computed hash — same decline the
        // builder form makes, for the same reason.
        Some(_) => {
            diags.push(residue(
                expr,
                "attributes are a computed expression, not a literal hash",
            ));
            return false;
        }
    };
    let span = expr.span;
    let mut parts: Vec<InterpPart> = vec![InterpPart::Text { value: format!("<{name}") }];
    append_attr_parts(&mut parts, &opts);
    parts.push(InterpPart::Text { value: " />".to_string() });
    let mut built = string_interp(parts);
    // A lone `ViewHelpers` resolves against the enclosing helper module
    // and raises NameError — same reason the two builder paths do this.
    qualify_view_helpers(&mut built);
    *expr = html_safe(built, span);
    true
}

fn rewrite_turbo_frame_tag(expr: &mut Expr, models: &std::collections::HashSet<String>) -> bool {
    let ExprNode::Send { recv: None, method, args, block, .. } = &*expr.node else {
        return false;
    };
    if method.as_str() != "turbo_frame_tag" {
        return false;
    }
    let is_record = |e: &Expr| turbo_frames::names_a_record(e, models);
    let Some(mut parts) = turbo_frames::frame_open_parts(args, &is_record) else {
        return false;
    };
    let span = expr.span;
    if let Some(block) = block.clone() {
        parts.push(InterpPart::Expr { expr: capture_call(block, span) });
    }
    parts.push(InterpPart::Text { value: turbo_frames::FRAME_CLOSE.to_string() });

    let mut built = string_interp(parts);
    qualify_view_helpers(&mut built);
    *expr = html_safe(built, span);
    true
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
///
/// A FORWARDED block (`tag.div …, &` / `turbo_frame_tag …, &`, which
/// ingest spells as the `__blk` binding) may be ABSENT at run time: the
/// helper declares an optional block and a caller can omit it, which is
/// exactly how campfire's room page reaches `sidebar_turbo_frame_tag(src:
/// user_sidebar_path)` with no block at all. Rails guards this in the tag
/// builder itself — `content = capture(&block) if block` — so the guard
/// belongs here rather than in nine runtime `capture`s, whose `yield`
/// would otherwise raise LocalJumpError on the frame that has no body.
fn capture_call(block: Expr, span: crate::span::Span) -> Expr {
    let call = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("capture"),
            args: vec![],
            block: Some(block.clone()),
            parenthesized: false,
        },
    );
    if !matches!(&*block.node, ExprNode::Var { .. }) {
        return call;
    }
    Expr::new(
        span,
        ExprNode::If {
            cond: Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(block),
                    method: Symbol::from("nil?"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
            then_branch: lit_str(String::new()),
            else_branch: call,
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
    let mut e = e;
    e.ty = Some(crate::ty::Ty::Str);
    let mut marked = Expr::new(
        span,
        ExprNode::Send {
            recv: Some(e),
            method: Symbol::from("html_safe"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    marked.ty = Some(crate::ty::Ty::Str);
    marked
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
    let mut built = html_safe(view_helpers_call("content_tag", args), span);
    // QUALIFY HERE, not at the call sites. `view_helpers_call` builds a bare
    // `ViewHelpers.<m>`, and the emitted tree defines the module only as
    // `ActionView::ViewHelpers` — there is no top-level alias. The inline
    // path qualifies its own result (six `qualify_view_helpers(&mut built)`
    // calls), but every fallback `return`s before reaching them.
    //
    // That was latent until a lone `**splat` first routed here: campfire's
    // `local_datetime_tag` emitted `ViewHelpers.content_tag(…)`, which
    // raises NameError — and `MessagesHelper#message_tag` wraps its body in
    // campfire's own `rescue Exception`, so the raise became an EMPTY
    // string and the message silently vanished from the page. That read as
    // `expected "#message_13" in response body`: two files, five tests, and
    // no mention of the constant anywhere in the failure.
    qualify_view_helpers(&mut built);
    built
}

/// The shape a double splat leaves behind: `head.merge({ … })`, chained
/// left-to-right, one link per splat boundary (`ingest::expr`).
///
/// Matched STRUCTURALLY rather than by asking whether the expression is
/// hash-typed, because this pass runs before that question has an answer
/// for a `**rest` parameter — the merge's head is the bare parameter, and
/// its type is whatever the caller passed.
///
/// Deliberately narrow: the receiver may be anything (it is the splatted
/// parameter), but the argument must be a Hash LITERAL, which is what the
/// desugar always builds. A hand-written `foo.merge(bar)` in a tag call
/// does not match, and keeps its old meaning.
fn is_kwsplat_merge_chain(e: &Expr) -> bool {
    match &*e.node {
        ExprNode::Send { recv: Some(recv), method, args, .. }
            if method.as_str() == "merge" && args.len() == 1 =>
        {
            matches!(&*args[0].node, ExprNode::Hash { .. }) || is_kwsplat_merge_chain(recv)
        }
        _ => false,
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::Span;

    fn ivar(name: &str) -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from(name) })
    }

    fn sym(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } },
        )
    }

    fn array(elements: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Array { elements, style: crate::expr::ArrayStyle::default() },
        )
    }

    fn set(names: &[&str]) -> std::collections::HashSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    fn hash(entries: Vec<(&str, Expr)>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Hash {
                entries: entries.into_iter().map(|(k, v)| (sym(k), v)).collect(),
                kwargs: false,
            },
        )
    }

    fn send(recv: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(recv),
                method: Symbol::from(method),
                args,
                block: None,
                parenthesized: true,
            },
        )
    }

    fn lvar(name: &str) -> Expr {
        Expr::new(
            Span::synthetic(),
            ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from(name) },
        )
    }

    /// The campfire site this exists for. `TimeHelper#local_datetime_tag`
    /// is `tag.time **attributes, datetime: …, data: { local_time_target: style }`,
    /// and the double splat leaves a `merge` chain — ONE argument, and a
    /// `Send` rather than a `Hash`. Read as content, it stringified the
    /// attribute hash into the tag's body, so every `<time>` campfire
    /// rendered came out as
    /// `<time>{datetime: "…", data: {local_time_target: :date}}</time>`:
    /// the day separator on every message, and every permalink timestamp.
    #[test]
    fn a_lone_kwsplat_merge_chain_is_attributes_not_content() {
        let chain = send(lvar("attributes"), "merge", vec![hash(vec![("datetime", sym("iso"))])]);
        assert!(
            is_kwsplat_merge_chain(&chain),
            "`attributes.merge({{…}})` is the shape ingest leaves a double splat in"
        );
    }

    /// Chained splats (`**a, k: 1, **b`) nest the merges, so the check has
    /// to walk the receiver rather than only inspecting the outermost call.
    #[test]
    fn a_nested_merge_chain_is_still_attributes() {
        let inner = send(lvar("a"), "merge", vec![hash(vec![("k", sym("v"))])]);
        let outer = send(inner, "merge", vec![lvar("b")]);
        assert!(is_kwsplat_merge_chain(&outer));
    }

    /// Narrow on purpose: a hand-written `merge` whose argument is not a
    /// hash literal is not the desugar's shape, and keeps its old meaning
    /// rather than being silently promoted to attributes.
    #[test]
    fn a_plain_merge_of_two_variables_is_not_the_desugar() {
        let call = send(lvar("a"), "merge", vec![lvar("b")]);
        assert!(!is_kwsplat_merge_chain(&call));
    }

    /// The campfire site this exists for: `link_to [ :edit, @room ]` in
    /// `RoomsHelper#link_to_edit_room`, which rendered the room's
    /// `inspect` into every room page's nav.
    /// No STI in play for most of these — the empty map is the plain
    /// (non-dispatching) shape.
    fn no_sti() -> std::collections::HashMap<String, Vec<String>> {
        Default::default()
    }

    #[test]
    fn a_prefixed_polymorphic_array_becomes_its_route_helper() {
        let url = array(vec![sym("edit"), ivar("room")]);
        let call =
            polymorphic_route_call(&url, &set(&["room"]), &set(&["edit_room_path"]), &no_sti())
                .expect("[:edit, @room] should resolve");
        let ExprNode::Send { recv: None, method, args, .. } = &*call.node else {
            panic!("expected a bare call, got {:?}", call.node);
        };
        assert_eq!(method.as_str(), "edit_room_path");
        // The RECORD is the argument, not its id: `route_helper_receiver`
        // owns that rewrite and applies it at emit.
        assert_eq!(args.len(), 1);
        assert!(matches!(&*args[0].node, ExprNode::Ivar { .. }));
    }

    #[test]
    fn a_bare_record_array_is_the_plain_helper() {
        let url = array(vec![ivar("room")]);
        let call = polymorphic_route_call(&url, &set(&["room"]), &set(&["room_path"]), &no_sti())
            .expect("[@room] should resolve");
        let ExprNode::Send { method, .. } = &*call.node else { panic!() };
        assert_eq!(method.as_str(), "room_path");
    }

    /// BOTH guards, and the reason each is there. An unknown model means
    /// the array was never a polymorphic URL; a known model whose route
    /// the app does not declare would emit a call to a helper nothing
    /// defines — a NameError on CRuby and a build wall on a strict
    /// target, strictly worse than the ledgered array it replaced.
    #[test]
    fn it_declines_rather_than_inventing_a_helper() {
        // Not a model.
        assert!(
            polymorphic_route_call(
                &array(vec![sym("edit"), ivar("widget")]),
                &set(&["room"]),
                &set(&["edit_room_path", "edit_widget_path"]),
                &no_sti(),
            )
            .is_none(),
            "a name that is not a model must not resolve"
        );
        // A model, but the app declares no such route.
        assert!(
            polymorphic_route_call(
                &array(vec![sym("edit"), ivar("room")]),
                &set(&["room"]),
                &set(&["room_path"]),
                &no_sti(),
            )
            .is_none(),
            "an undeclared route must not be invented"
        );
        // A prefix that is not a symbol carries no name to join.
        assert!(
            polymorphic_route_call(
                &array(vec![ivar("scope"), ivar("room")]),
                &set(&["room", "scope"]),
                &set(&["edit_room_path"]),
                &no_sti(),
            )
            .is_none(),
            "a non-symbol prefix must not resolve"
        );
    }

    /// An STI base names its route through the record's RUNTIME class:
    /// campfire's room 1 is a `Rooms::Open`, so Rails' `[ :edit, @room ]`
    /// answers `/rooms/opens/1/edit`. The resolved call is a nested `if`
    /// chain over `dom_prefix()` — the same type-column dispatch the dom
    /// id rides, spelled `if` because the weakest emitter has no `case`
    /// arm — ending in the base helper.
    #[test]
    fn an_sti_base_dispatches_on_its_subclass_stems() {
        let mut sti = std::collections::HashMap::new();
        sti.insert(
            "room".to_string(),
            vec!["rooms_open".to_string(), "rooms_closed".to_string()],
        );
        let call = polymorphic_route_call(
            &array(vec![sym("edit"), ivar("room")]),
            &set(&["room"]),
            &set(&["edit_room_path", "edit_rooms_open_path", "edit_rooms_closed_path"]),
            &sti,
        )
        .expect("[:edit, @room] should resolve");
        let ExprNode::If { cond, then_branch, else_branch } = &*call.node else {
            panic!("expected a dom_prefix dispatch, got {:?}", call.node);
        };
        let ExprNode::Send { recv: Some(recv), method, .. } = &*cond.node else {
            panic!("expected a `dom_prefix() == \"…\"` condition")
        };
        assert_eq!(method.as_str(), "==");
        let ExprNode::Send { method, parenthesized, .. } = &*recv.node else {
            panic!("expected a dom_prefix() send")
        };
        assert_eq!(method.as_str(), "dom_prefix");
        assert!(*parenthesized, "zero-arg send needs parens (TS emit)");
        let ExprNode::Send { method, .. } = &*then_branch.node else { panic!() };
        assert_eq!(method.as_str(), "edit_rooms_open_path");
        let ExprNode::If { then_branch: second, else_branch: last, .. } = &*else_branch.node
        else {
            panic!("expected the second subclass arm");
        };
        let ExprNode::Send { method, .. } = &*second.node else { panic!() };
        assert_eq!(method.as_str(), "edit_rooms_closed_path");
        let ExprNode::Send { method, .. } = &*last.node else { panic!() };
        assert_eq!(method.as_str(), "edit_room_path");
    }

    /// A subclass whose helper the app does not declare folds into the
    /// base arm rather than inventing a call — and when NO subclass has
    /// a route, the plain base call comes back with no dispatch at all.
    #[test]
    fn a_routeless_subclass_folds_into_the_base_arm() {
        let mut sti = std::collections::HashMap::new();
        sti.insert("room".to_string(), vec!["rooms_open".to_string()]);
        let call = polymorphic_route_call(
            &array(vec![sym("edit"), ivar("room")]),
            &set(&["room"]),
            &set(&["edit_room_path"]),
            &sti,
        )
        .expect("[:edit, @room] should resolve");
        let ExprNode::Send { method, .. } = &*call.node else {
            panic!("with no routable subclass the base call should be plain");
        };
        assert_eq!(method.as_str(), "edit_room_path");
    }
}
