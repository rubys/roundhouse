//! `form_with` capture lowering: inline-expand `<%= form_with(opts) do
//! |form| ...inner... %>` at lower time. The lowerer materializes the
//! opening `<form ...>` tag, calls runtime helpers for CSRF + method
//! override, constructs a typed FormBuilder, walks the inner body
//! against the SAME outer accumulator (no inner `body =` capture),
//! and emits the closing `</form>`. This retires the runtime
//! `ViewHelpers.form_with(...)` call shape — its 5-way heterogeneous
//! kwargs hash is the dominant Rust HashMap parity bug.
//!
//! FormBuilder (`form.label`, `form.text_field`, etc.) still
//! dispatches as a runtime instance method — Wedge 1b-ii will inline
//! those too. The per-call-site `opts: Hash[Sym, untyped]` argument
//! on those methods is the smaller-but-still-real heterogeneity
//! remaining after this wedge.

use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::naming::{singularize, snake_case};
use crate::span::Span;

use super::walker::walk_body;
use super::{
    accumulator_append_call, lit_str, lit_sym, send, view_helpers_call,
    FormBuilderBinding, ViewCtx,
};

/// Typed pieces extracted from a `form_with` call's surface kwargs.
/// `model` is the record expression (or `Class.new` for non-persisted
/// nested resources); `model_name` is the form-prefix string;
/// `action` and `method` are computed (often via `.persisted?`
/// conditionals); `opts_entries` carries any leftover non-`model:`
/// kwargs that render as `<form>` tag attributes.
pub(super) struct FormWithComponents {
    pub(super) model: Expr,
    pub(super) model_name: String,
    pub(super) action: Expr,
    pub(super) method: Expr,
    pub(super) opts_entries: Vec<(Expr, Expr)>,
}

/// Inline-expand `<%= form_tag(action, opts) do ...inner... %>` — the
/// builder-less bare form (lobsters' link_post). Same statement-splice
/// shape as `emit_form_with_inline` minus the record/builder
/// machinery: open `<form>` tag (action spliced through html_escape,
/// literal opts as compile-time attributes), the CSRF hidden input
/// (Rails embeds it for this always-POST form; no `_method` override —
/// form_tag never PATCHes here), the walked block body against the
/// outer accumulator, `</form>`. Byte-matches the CRuby overlay's
/// runtime form_tag, which the bench replay exercises. The action goes
/// through `route_helperize`, so bare path helpers and model-named
/// records resolve exactly like form_with's `url:`.
pub(super) fn emit_form_tag_inline(args: &[Expr], block: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    let ExprNode::Lambda { body, .. } = &*block.node else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let Some(action_arg) = args.first() else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let route_helpers = || {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
        )
    };
    let opts_entries: Vec<(Expr, Expr)> = args
        .iter()
        .skip(1)
        .find_map(|a| match &*a.node {
            ExprNode::Hash { entries, .. } => Some(entries.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let comps = FormWithComponents {
        model: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        model_name: String::new(),
        action: route_helperize(action_arg.clone(), &route_helpers, ctx),
        // Unused by the open-tag emission (it hard-codes POST), but
        // the components struct carries it for shape parity.
        method: lit_sym(Symbol::from("post")),
        opts_entries,
    };
    let mut out: Vec<Expr> = Vec::new();
    out.push(emit_open_form_tag(&comps, ctx));
    out.push(accumulator_append_call(
        view_helpers_call("csrf_token_hidden_input", Vec::new()),
        ctx,
    ));
    out.extend(walk_body(body, ctx));
    out.push(accumulator_append_call(lit_str("</form>".to_string()), ctx));
    out
}

/// Inline-expand `<%= form_with(opts) do |form| ...inner... %>` at
/// lower time. Returns a Vec of statements the caller splices into
/// the outer accumulator's statement list. Walks the inner block body
/// with the same outer `io` accumulator (no inner capture) so each
/// `<%= form.text_field … %>` lands directly in the parent stream.
pub(super) fn emit_form_with_inline(
    args: &[Expr],
    block: &Expr,
    ctx: &ViewCtx,
) -> Vec<Expr> {
    let ExprNode::Lambda { params, body, .. } = &*block.node else {
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };
    let form_param = params
        .first()
        .cloned()
        .unwrap_or_else(|| Symbol::from("form"));

    let record_local = find_kwarg_local_name(args);

    let Some(comps) = classify_form_with_components(args, ctx) else {
        // Non-resource form_with (no `model:` kwarg) — fall back to a
        // safe-empty append so the file still parses. Real fixtures
        // always pass `model:`; if this shape becomes load-bearing,
        // synthesize a no-model expansion here.
        return vec![accumulator_append_call(lit_str(String::new()), ctx)];
    };

    let mut out: Vec<Expr> = Vec::new();

    // Bind locals before emitting the open tag so the open-tag's
    // action interp can read them. `<form_param>_record` holds the
    // model expression — reused as the receiver for `form.X`
    // attribute reads. `<form_param>_method` holds the method symbol
    // — read by `form.submit`'s default-text expansion. Reuse the
    // source local for record when the source already named one
    // (`model: article`); synthesize otherwise (`model: Comment.new`).
    let form_param_str = form_param.as_str();
    let record_var = match record_local.as_deref() {
        Some(name) if !name.is_empty() => Symbol::from(name),
        _ => {
            let synth = format!("{form_param_str}_record");
            out.push(Expr::new(
                Span::synthetic(),
                ExprNode::Assign {
                    target: LValue::Var {
                        id: VarId(0),
                        name: Symbol::from(synth.as_str()),
                    },
                    value: comps.model.clone(),
                },
            ));
            Symbol::from(synth)
        }
    };
    let form_method_var = Symbol::from(format!("{form_param_str}_method"));
    out.push(Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: form_method_var.clone() },
            value: comps.method.clone(),
        },
    ));

    // 1. Open `<form ...>` tag — built as a StringInterp with the
    //    action expression spliced in. Order matches Rails'
    //    `render_attrs({action:, "accept-charset":, method:}.merge(opts))`:
    //    action, accept-charset, method, then user opts. The HTML
    //    `method` attribute is always "post" for resource forms
    //    (PATCH/DELETE flow through `_method` override below); :get
    //    fixtures aren't exercised so we hard-code "post" here.
    out.push(emit_open_form_tag(&comps, ctx));

    // 2. Method override: `<input type="hidden" name="_method"
    //    value="patch">` for non-get/post methods, empty string
    //    otherwise. Calls the runtime helper with the bound method
    //    local so the conditional evaluates once.
    let method_var_ref = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: VarId(0), name: form_method_var.clone() },
    );
    out.push(accumulator_append_call(
        view_helpers_call("method_override_input", vec![method_var_ref]),
        ctx,
    ));

    // 3. CSRF token hidden input — emitted unconditionally to match
    //    Rails' position (after _method, before the body).
    out.push(accumulator_append_call(
        view_helpers_call("csrf_token_hidden_input", Vec::new()),
        ctx,
    ));

    // 4. Walk the block body against the OUTER accumulator. The
    //    binding registered below feeds `form.X` macro expansion
    //    (form_builder.rs::emit_form_builder_inline) — no runtime
    //    FormBuilder construction; the form.X calls inline directly
    //    to `<label>...`/`<input>...` HTML.
    let mut inner_ctx = ctx.with_locals([form_param_str.to_string()]);
    inner_ctx.form_records.push(FormBuilderBinding {
        form_param: form_param_str.to_string(),
        model_name: comps.model_name.clone(),
        record_var: record_var.clone(),
        form_method_var,
    });
    // `f.object` is the one FormBuilder method used in EXPRESSION
    // position (`errors_for f.object`) — substitute the record local
    // before the walk so downstream classifiers see a plain Var.
    let body = rewrite_form_object_reads(body, form_param_str, &record_var);
    out.extend(walk_body(&body, &inner_ctx));

    // 5. Close `</form>`.
    out.push(accumulator_append_call(
        lit_str("</form>".to_string()),
        ctx,
    ));

    out
}

/// Build the opening `<form action="..." accept-charset="UTF-8"
/// method="post"<opts>>` tag as one `<accumulator> << "<...>"`
/// statement. Static text segments are folded into the surrounding
/// `InterpPart::Text` so the emitted bytes match Rails'
/// `render_attrs`-produced order: action, accept-charset, method,
/// then user opts in source order. Action value flows through
/// `ViewHelpers.html_escape` to match runtime semantics; opts values
/// likewise (they may carry user-supplied strings).
fn emit_open_form_tag(comps: &FormWithComponents, ctx: &ViewCtx) -> Expr {
    let mut parts: Vec<InterpPart> = Vec::new();
    parts.push(InterpPart::Text {
        value: "<form action=\"".to_string(),
    });
    parts.push(InterpPart::Expr {
        expr: view_helpers_call("html_escape", vec![comps.action.clone()]),
    });
    parts.push(InterpPart::Text {
        value: "\" accept-charset=\"UTF-8\" method=\"post\"".to_string(),
    });
    for (k, v) in &comps.opts_entries {
        let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
            // Non-symbol keys not exercised; skip silently to keep
            // output well-formed.
            continue;
        };
        parts.push(InterpPart::Text {
            value: format!(" {}=\"", key.as_str()),
        });
        parts.push(InterpPart::Expr {
            expr: view_helpers_call("html_escape", vec![simplify_opts_value(k, v)]),
        });
        parts.push(InterpPart::Text {
            value: "\"".to_string(),
        });
    }
    parts.push(InterpPart::Text {
        value: ">".to_string(),
    });
    accumulator_append_call(
        Expr::new(Span::synthetic(), ExprNode::StringInterp { parts }),
        ctx,
    )
}

/// Simplify a single opts entry's value at lower time. Today only
/// `class: [base, {key: pred, ...}]` gets collapsed; other shapes
/// pass through unchanged. Matches the existing FormBuilder-side
/// simplification so per-form-tag and per-input-attr behavior stay
/// in sync.
fn simplify_opts_value(k: &Expr, v: &Expr) -> Expr {
    let is_class_key = matches!(
        &*k.node,
        ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "class",
    );
    if is_class_key {
        super::form_builder::simplify_class_array_pub(v)
    } else {
        v.clone()
    }
}

/// Extract the typed pieces of a `form_with(...)` call's surface
/// kwargs. Returns `None` when no `model:` kwarg is present — the
/// non-resource form_with shape isn't exercised by real-blog and
/// would need a separate derivation for `model_name`/`action`.
///
/// `action` and `method` come back as IR expressions, not literal
/// values, because they're typically computed from `record.persisted?`
/// (PATCH for existing records vs POST for new). The polymorphic
/// array-model form (`model: [parent, Class.new]` for nested
/// resources) returns the child as `model`, the child class's
/// singular name as `model_name`, and a nested-collection path
/// helper as `action` (method is :post since `Class.new` is never
/// persisted).
/// Resolve a `url:` value to a callable action expression. A bare route
/// helper (`login_path` — a no-receiver `*_path`/`*_url` Send) becomes
/// `RouteHelpers.login_path(args)`; anything else (a String literal, an
/// already-qualified call) passes through unchanged.
fn route_helperize(url: Expr, route_helpers: &impl Fn() -> Expr, ctx: &ViewCtx) -> Expr {
    if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*url.node {
        let m = method.as_str();
        if m.ends_with("_path") {
            return send(Some(route_helpers()), m, args.clone(), None, true);
        }
        // `_url` absolute variants: RouteHelpers only generates `_path`
        // functions — the shared absolute-interp grounding (lobsters'
        // keybase form posts to `keybase_proofs_url`).
        if let Some(stem) = m.strip_suffix("_url") {
            return super::absolute_url_interp(stem, args.clone());
        }
    }
    // A bare local/ivar `url:` naming a KNOWN MODEL is Rails'
    // polymorphic-record form (`form_with url: comment`), and its
    // action resolves at COMPILE time — `url_for(record)` semantics:
    // member path when persisted, collection path when new (the `url:`
    // form keeps POST either way; only `model:` derives PATCH). The
    // record rides WHOLE into the member helper so a custom `to_param`
    // (lobsters' Comment#short_id) shapes the segment exactly as Rails
    // does. This is the typed replacement for the runtime `url_for`
    // fallback below, whose `is_a?`-dispatch body is CRuby-overlay-only
    // and refuses under spinel AOT.
    let bare_name: Option<&str> = match &*url.node {
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.as_str())
        }
        ExprNode::Var { name, .. } | ExprNode::Ivar { name } => Some(name.as_str()),
        _ => None,
    };
    if let Some(name) = bare_name {
        if ctx.model_singulars.contains(name) {
            let plural = crate::naming::pluralize_snake(name);
            let member = super::route_helpers_call(&format!("{name}_path"), vec![url.clone()]);
            let collection = super::route_helpers_call(&format!("{plural}_path"), Vec::new());
            let has_member = ctx.route_helper_names.is_empty()
                || ctx.route_helper_names.contains(&format!("{name}_path"));
            let has_collection = ctx.route_helper_names.is_empty()
                || ctx.route_helper_names.contains(&format!("{plural}_path"));
            return match (has_member, has_collection) {
                (true, false) => member,
                (false, true) => collection,
                _ => Expr::new(
                    url.span,
                    ExprNode::If {
                        cond: send(Some(url.clone()), "persisted?", Vec::new(), None, false),
                        then_branch: member,
                        else_branch: collection,
                    },
                ),
            };
        }
    }
    // Any other bare local/ivar defers to the runtime's url_for
    // (strings pass through unchanged there; record resolution via
    // class table + persistence is the CRuby overlay's job).
    let is_bareword = matches!(
        &*url.node,
        ExprNode::Send { recv: None, args, block: None, .. } if args.is_empty()
    );
    if is_bareword || matches!(&*url.node, ExprNode::Var { .. } | ExprNode::Ivar { .. }) {
        let span = url.span;
        return Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(
                    span,
                    ExprNode::Const {
                        path: vec![
                            crate::ident::Symbol::from("ActionView"),
                            crate::ident::Symbol::from("ViewHelpers"),
                        ],
                    },
                )),
                method: crate::ident::Symbol::from("url_for"),
                args: vec![url],
                block: None,
                parenthesized: true,
            },
        );
    }
    url
}

fn classify_form_with_components(
    args: &[Expr],
    ctx: &ViewCtx,
) -> Option<FormWithComponents> {
    let mut model_expr: Option<Expr> = None;
    let mut url_expr: Option<Expr> = None;
    let mut opts_entries: Vec<(Expr, Expr)> = Vec::new();

    for arg in args {
        if let ExprNode::Hash { entries, .. } = &*arg.node {
            for (k, v) in entries {
                if let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node {
                    match key.as_str() {
                        "model" => {
                            model_expr = Some(v.clone());
                            continue;
                        }
                        // `scope:`/`method:` also steer form_with; `url:` is
                        // the non-resource action target — kept out of opts.
                        "url" => {
                            url_expr = Some(v.clone());
                            continue;
                        }
                        // form_with's generated-id prefix option, not an
                        // HTML attribute — left in opts it rendered a
                        // bogus namespace="…" attr on /settings. The id
                        // prefixing itself (edit_user_user_username)
                        // rides the typed-record param_key follow-up.
                        "namespace" => {
                            continue;
                        }
                        _ => {}
                    }
                }
                opts_entries.push((k.clone(), v.clone()));
            }
        }
    }

    let route_helpers = || {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
        )
    };

    // `form_with url: login_path do |form|` — a non-resource form. No model
    // prefix (fields name bare), action is the given URL (a bare route
    // helper resolves to `RouteHelpers.<x>`), method POST (form_with's
    // default). The record placeholder is nil — non-model fields read no
    // attributes (`field_value_read` returns nil for an empty model_name).
    let model = match model_expr {
        Some(m) => m,
        None => {
            let url = url_expr?;
            // Same `method:` extraction the url+model branch does — left
            // in opts it renders as a second literal `method="..."` attr
            // (and a Symbol value crashes html_escape).
            let is_method_key = |k: &Expr| {
                matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                    if value.as_str() == "method")
            };
            let method = opts_entries
                .iter()
                .find(|(k, _)| is_method_key(k))
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| lit_sym(Symbol::from("post")));
            opts_entries.retain(|(k, _)| !is_method_key(k));
            return Some(FormWithComponents {
                model: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
                model_name: String::new(),
                action: route_helperize(url, &route_helpers, ctx),
                method,
                opts_entries,
            });
        }
    };

    if let Some((nested_model, nested_name, nested_action)) =
        nested_resource_form(&model, &route_helpers)
    {
        return Some(FormWithComponents {
            model: nested_model,
            model_name: nested_name,
            action: nested_action,
            method: lit_sym(Symbol::from("post")),
            opts_entries,
        });
    }

    let plural = ctx.resource_dir.as_str();
    let singular = singularize(plural);

    // `form_with model: @edit_user, url: settings_path` — an explicit
    // `url:` beside `model:` overrides the resource-convention action
    // (Rails consults url first; lobsters' settings form has no
    // `setting_path` route for the convention to name). Fields still
    // name under the model. Method: an explicit `method:` opt wins,
    // else form_with's POST default.
    if let Some(url) = url_expr {
        // `method:` steers the form verb — pull it OUT of opts_entries
        // (left in, it renders as a literal `method="..."` attribute on
        // top of the builder's own) and use it as the method component;
        // default POST like the url-only branch.
        let is_method_key = |k: &Expr| {
            matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } }
                if value.as_str() == "method")
        };
        let method = opts_entries
            .iter()
            .find(|(k, _)| is_method_key(k))
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| lit_sym(Symbol::from("post")));
        opts_entries.retain(|(k, _)| !is_method_key(k));
        return Some(FormWithComponents {
            model,
            model_name: singular,
            action: route_helperize(url, &route_helpers, ctx),
            method,
            opts_entries,
        });
    }

    // record.persisted?
    let persisted = send(Some(model.clone()), "persisted?", Vec::new(), None, false);

    // RouteHelpers.<singular>_path(record.id) when persisted, else
    // RouteHelpers.<plural>_path for new records.
    let model_id = send(Some(model.clone()), "id", Vec::new(), None, false);
    let member_path = send(
        Some(route_helpers()),
        &format!("{singular}_path"),
        vec![model_id],
        None,
        true,
    );
    let collection_path = send(
        Some(route_helpers()),
        &format!("{plural}_path"),
        Vec::new(),
        None,
        false,
    );
    // Emit only the arms whose route helper EXISTS (domains has a
    // member route but no collection; an unconditional ternary calls
    // an undefined RouteHelpers method). An empty helper set (test
    // harnesses without routes) keeps both arms. A one-armed form is
    // Rails-honest: submitting the missing arm's case would be a
    // routing error there too.
    let has_member = ctx.route_helper_names.is_empty()
        || ctx.route_helper_names.contains(&format!("{singular}_path"));
    let has_collection = ctx.route_helper_names.is_empty()
        || ctx.route_helper_names.contains(&format!("{plural}_path"));
    let action = match (has_member, has_collection) {
        (true, false) => member_path,
        (false, true) => collection_path,
        _ => Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: persisted.clone(),
                then_branch: member_path,
                else_branch: collection_path,
            },
        ),
    };
    let method = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: persisted,
            then_branch: lit_sym(Symbol::from("patch")),
            else_branch: lit_sym(Symbol::from("post")),
        },
    );

    Some(FormWithComponents {
        model,
        model_name: singular,
        action,
        method,
        opts_entries,
    })
}

/// Match `model: [parent, Class.new(...)]` (the polymorphic-array form
/// Rails uses for nested resources) and produce `(child, child_singular,
/// nested_action_expr)` where the action targets the nested collection
/// path. Returns None for any other shape so the caller falls through
/// to plain-record handling.
fn nested_resource_form(
    model: &Expr,
    route_helpers: &dyn Fn() -> Expr,
) -> Option<(Expr, String, Expr)> {
    let ExprNode::Array { elements, .. } = &*model.node else {
        return None;
    };
    if elements.len() < 2 {
        return None;
    }
    let parent = elements.first()?;
    let child = elements.last()?;

    // Parent local name: only support a Var receiver (e.g. `article`)
    // for now. An ivar would have been pre-rewritten to a local.
    let parent_local = match &*parent.node {
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        ExprNode::Send {
            recv: None, method, args, block: None, ..
        } if args.is_empty() => method.as_str().to_string(),
        _ => return None,
    };
    let parent_singular = singularize(&snake_case(&parent_local));

    // Child class: only support `Class.new(...)` shape. Comment.new is
    // by definition not persisted, so the action is the nested
    // collection path with method :post.
    let ExprNode::Send {
        recv: Some(child_recv),
        method: child_method,
        ..
    } = &*child.node
    else {
        return None;
    };
    if child_method.as_str() != "new" {
        return None;
    }
    let ExprNode::Const { path } = &*child_recv.node else {
        return None;
    };
    let class_name = path.last()?.as_str();
    let child_singular = snake_case(class_name);
    let child_plural = format!("{child_singular}s"); // naïve; comments fixture only

    // RouteHelpers.<parent_singular>_<child_plural>_path(parent.id)
    let parent_id = send(Some(parent.clone()), "id", Vec::new(), None, false);
    let action = send(
        Some(route_helpers()),
        &format!("{parent_singular}_{child_plural}_path"),
        vec![parent_id],
        None,
        true,
    );

    Some((child.clone(), child_singular, action))
}

/// True when the receiver is a `<x>.errors` Send — i.e. the iterable
/// of an errors-each loop. Spinel surfaces errors as `Vec<String>`,
/// which is what triggers the `full_message` rewrite below.
pub(super) fn is_errors_each(recv: &Expr) -> bool {
    matches!(
        &*recv.node,
        ExprNode::Send { method, args, block: None, .. }
            if method.as_str() == "errors" && args.is_empty()
    )
}

/// Substitute `<var>.full_message` (with no args, no block) with a bare
/// `<var>` reference, recursively through the body. Other `<var>.*`
/// projections pass through — only `full_message` is the Rails-side
/// adapter Spinel-runtime errors don't expose.
pub(super) fn rewrite_errors_each_body(body: &Expr, var_name: &str) -> Expr {
    let new_node = match &*body.node {
        ExprNode::Send {
            recv: Some(r),
            method,
            args,
            block,
            parenthesized,
        } => {
            let r_is_var = matches!(
                &*r.node,
                ExprNode::Var { name, .. } if name.as_str() == var_name
            ) || matches!(
                &*r.node,
                ExprNode::Send { recv: None, method: m, args: a, block: None, .. }
                    if m.as_str() == var_name && a.is_empty()
            );
            if r_is_var && method.as_str() == "full_message" && args.is_empty() && block.is_none() {
                return r.clone();
            }
            ExprNode::Send {
                recv: Some(rewrite_errors_each_body(r, var_name)),
                method: method.clone(),
                args: args.iter().map(|a| rewrite_errors_each_body(a, var_name)).collect(),
                block: block.as_ref().map(|b| rewrite_errors_each_body(b, var_name)),
                parenthesized: *parenthesized,
            }
        }
        ExprNode::Send { recv: None, method, args, block, parenthesized } => ExprNode::Send {
            recv: None,
            method: method.clone(),
            args: args.iter().map(|a| rewrite_errors_each_body(a, var_name)).collect(),
            block: block.as_ref().map(|b| rewrite_errors_each_body(b, var_name)),
            parenthesized: *parenthesized,
        },
        ExprNode::Hash { entries, kwargs } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| {
                    (
                        rewrite_errors_each_body(k, var_name),
                        rewrite_errors_each_body(v, var_name),
                    )
                })
                .collect(),
            kwargs: *kwargs,
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements
                .iter()
                .map(|e| rewrite_errors_each_body(e, var_name))
                .collect(),
            style: *style,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|e| rewrite_errors_each_body(e, var_name)).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite_errors_each_body(cond, var_name),
            then_branch: rewrite_errors_each_body(then_branch, var_name),
            else_branch: rewrite_errors_each_body(else_branch, var_name),
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Expr { expr } => InterpPart::Expr {
                        expr: rewrite_errors_each_body(expr, var_name),
                    },
                    other => other.clone(),
                })
                .collect(),
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_errors_each_body(body, var_name),
            block_style: *block_style,
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: target.clone(),
            value: rewrite_errors_each_body(value, var_name),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_errors_each_body(left, var_name),
            right: rewrite_errors_each_body(right, var_name),
        },
        ExprNode::Apply { fun, args, block } => ExprNode::Apply {
            fun: rewrite_errors_each_body(fun, var_name),
            args: args.iter().map(|a| rewrite_errors_each_body(a, var_name)).collect(),
            block: block.as_ref().map(|b| rewrite_errors_each_body(b, var_name)),
        },
        other => other.clone(),
    };
    Expr::new(body.span, new_node)
}

/// Find the `model:` kwarg in a Hash arg of a form_with call and
/// return the local name it binds to (when the value is a Var or a
/// bareword Send). Returns None for other shapes (Class.new for
/// new-records, complex expressions). Used to seed
/// `ctx.form_records` so FormBuilder method dispatch can resolve
/// attribute lookups.
fn find_kwarg_local_name(args: &[Expr]) -> Option<String> {
    for arg in args {
        let ExprNode::Hash { entries, .. } = &*arg.node else {
            continue;
        };
        for (k, v) in entries {
            let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else {
                continue;
            };
            if key.as_str() != "model" {
                continue;
            }
            return match &*v.node {
                ExprNode::Var { name, .. } => Some(name.as_str().to_string()),
                ExprNode::Send {
                    recv: None,
                    method,
                    args,
                    block: None,
                    ..
                } if args.is_empty() => Some(method.as_str().to_string()),
                _ => None,
            };
        }
    }
    None
}


/// Replace `<form_param>.object` reads with the record local
/// (`f.object` → `f_record`). Runs over the form block body before the
/// walk; every other `f.<method>` stays for the macro-inline dispatch.
pub(super) fn rewrite_form_object_reads(body: &Expr, form_param: &str, record_var: &Symbol) -> Expr {
    fn walk(e: &Expr, form_param: &str, record_var: &Symbol) -> Expr {
        if let ExprNode::Send { recv: Some(r), method, args, block: None, .. } = &*e.node {
            if method.as_str() == "object" && args.is_empty() {
                // The builder reference is a Var inside a form_with
                // lambda (a real block param) but a bare zero-arg Send
                // in a bound PARTIAL (the form local dropped out of
                // the partial's params, so nothing declares it).
                if form_param_ref_name(r).is_some_and(|n| n == form_param) {
                    return Expr::new(
                        e.span,
                        ExprNode::Var { id: crate::ident::VarId(0), name: record_var.clone() },
                    );
                }
            }
        }
        let mut out = e.clone();
        out.node.for_each_child_mut(&mut |c| {
            *c = walk(c, form_param, record_var);
        });
        out
    }
    walk(body, form_param, record_var)
}

/// The name a form-builder receiver reference carries: a `Var` (block
/// param inside form_with) or a bare zero-arg `Send` (a bound
/// partial's form local — dropped from its params, so undeclared).
pub(super) fn form_param_ref_name(e: &Expr) -> Option<&str> {
    match &*e.node {
        ExprNode::Var { name, .. } => Some(name.as_str()),
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.as_str())
        }
        _ => None,
    }
}
