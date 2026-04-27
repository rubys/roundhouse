//! Lower a `View` (compiled-ERB IR) into a `LibraryClass` whose body is
//! one `module_function`-style class method per view, with bodies in
//! spinel-blog shape:
//!
//!   io = String.new
//!   io << ViewHelpers.turbo_stream_from("articles")
//!   ViewHelpers.content_for_set(:title, "Articles")
//!   if !articles.empty?
//!     articles.each { |a| io << Views::Articles.article(a) }
//!   end
//!   io
//!
//! Helper-call rewrites (`turbo_stream_from` → `ViewHelpers.turbo_stream_from`,
//! `link_to text, url` → `ViewHelpers.link_to(text, RouteHelpers.<x>_path(...))`,
//! auto-escape on bare interpolation, …) and render-partial dispatch
//! happen here so per-target emitters consume canonical IR — the same
//! rationale as `model_to_library` and `controller_to_library`.
//!
//! Scope of this first slice: the helpers needed by `articles/index.html.erb`
//! (turbo_stream_from, content_for setter, link_to with path-helper URL,
//! render @collection, `.any?`-style predicates, html_escape on bare
//! interpolation). FormBuilder/form_with capture, content_for capture,
//! errors-field predicates, and conditional-class composition land in
//! follow-on slices once their forcing fixtures are exercised.

use crate::App;
use crate::dialect::{LibraryClass, MethodDef, MethodReceiver, View};
use crate::effect::EffectSet;
use crate::expr::{BlockStyle, Expr, ExprNode, InterpPart, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::naming::{camelize, singularize, snake_case};
use crate::span::Span;

use super::view::{
    classify_form_builder_method, classify_nested_url_element, classify_render_partial,
    classify_view_helper, classify_view_url_arg, FormBuilderMethod, NestedUrlElement,
    RenderPartial, ViewHelperKind, ViewUrlArg,
};

/// Entry point. Turn one `View` into a one-method `LibraryClass`.
/// `app` is consulted only for known model names (so view args can be
/// typed implicitly downstream) and for FK resolution; the lowering is
/// otherwise pure.
pub fn lower_view_to_library_class(view: &View, app: &App) -> LibraryClass {
    let (dir, base) = split_view_name(view.name.as_str());
    let stem = base.trim_start_matches('_');

    let module_id = view_module_id(dir);
    let method_name = Symbol::from(stem);

    let known_models: Vec<String> =
        app.models.iter().map(|m| m.name.0.as_str().to_string()).collect();
    let arg_name = infer_view_arg(stem, dir, base.starts_with('_'), &known_models);

    // Rewrite `@ivar` → bare `ivar` everywhere so the inferred arg name
    // (and any extra params we surface) read as plain locals in the
    // emitted body. Mirrors the controller-side ivar-to-local pass.
    let rewritten = rewrite_ivars_to_locals(&view.body);

    // Collect free names other than the inferred arg → those become
    // additional positional params. Today this picks up `notice`,
    // `alert`, etc. (Rails flash helpers parsed as bare Sends/Vars).
    let extra_params = collect_extra_params(&rewritten, &arg_name);

    let mut params: Vec<Symbol> = Vec::new();
    if !arg_name.is_empty() {
        params.push(Symbol::from(arg_name.clone()));
    }
    for n in &extra_params {
        params.push(Symbol::from(n.clone()));
    }

    let mut locals: Vec<String> = Vec::new();
    if !arg_name.is_empty() {
        locals.push(arg_name.clone());
    }
    locals.extend(extra_params.iter().cloned());

    let ctx = ViewCtx {
        locals,
        arg_name: arg_name.clone(),
        resource_dir: dir.to_string(),
        accumulator: "io".to_string(),
        form_records: Vec::new(),
    };

    let mut body_stmts: Vec<Expr> = Vec::new();
    body_stmts.push(assign_accumulator_string_new(&ctx.accumulator));
    body_stmts.extend(walk_body(&rewritten, &ctx));
    body_stmts.push(var_ref(Symbol::from(ctx.accumulator.as_str())));

    let body = seq(body_stmts);

    let method = MethodDef {
        name: method_name,
        receiver: MethodReceiver::Class,
        params,
        body,
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(module_id.0.clone()),
    };

    LibraryClass {
        name: module_id,
        is_module: true,
        parent: None,
        includes: Vec::new(),
        methods: vec![method],
    }
}

// ── view-name → module / arg / method helpers ────────────────────

fn split_view_name(name: &str) -> (&str, &str) {
    name.rsplit_once('/').unwrap_or(("", name))
}

/// Module the view's method lives under: `Views::Articles` for an
/// `articles/...` view. Empty `dir` (uncommon — top-level view) maps
/// to the bare `Views` module.
fn view_module_id(dir: &str) -> ClassId {
    if dir.is_empty() {
        return ClassId(Symbol::from("Views"));
    }
    let camelized = camelize(&snake_case(dir));
    ClassId(Symbol::from(format!("Views::{camelized}")))
}

/// Pick the single positional parameter name for a view. Action views
/// (`articles/index`) take the plural collection (`articles`); show /
/// new / edit / create / update / destroy + partials take the singular
/// (`article`). Layouts take `body` (the rendered inner-view string;
/// bare `yield` in the layout source resolves to this local). Top-
/// level views with no resource directory fall back to an empty arg
/// name (no positional param).
fn infer_view_arg(stem: &str, dir: &str, is_partial: bool, _known_models: &[String]) -> String {
    if dir.is_empty() {
        return String::new();
    }
    if dir == "layouts" {
        return "body".to_string();
    }
    if is_partial {
        return singularize(dir);
    }
    match stem {
        "index" => dir.to_string(),
        _ => singularize(dir),
    }
}

// ── ivar → local rewrite ─────────────────────────────────────────

/// Rewrite every `@ivar` read (and Ivar-LValue assign) under `expr`
/// into a bare `Var` of the same name. The inferred view arg + any
/// extra params resolve to those rewritten Vars in the emitted body.
fn rewrite_ivars_to_locals(expr: &Expr) -> Expr {
    let new_node = match &*expr.node {
        ExprNode::Ivar { name } => ExprNode::Var { id: VarId(0), name: name.clone() },
        ExprNode::Assign { target: LValue::Ivar { name }, value } => ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: name.clone() },
            value: rewrite_ivars_to_locals(value),
        },
        ExprNode::Assign { target, value } => ExprNode::Assign {
            target: rewrite_lvalue(target),
            value: rewrite_ivars_to_locals(value),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_ivars_to_locals),
            method: method.clone(),
            args: args.iter().map(rewrite_ivars_to_locals).collect(),
            block: block.as_ref().map(rewrite_ivars_to_locals),
            parenthesized: *parenthesized,
        },
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(rewrite_ivars_to_locals).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: rewrite_ivars_to_locals(cond),
            then_branch: rewrite_ivars_to_locals(then_branch),
            else_branch: rewrite_ivars_to_locals(else_branch),
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_ivars_to_locals(left),
            right: rewrite_ivars_to_locals(right),
        },
        ExprNode::Array { elements, style } => ExprNode::Array {
            elements: elements.iter().map(rewrite_ivars_to_locals).collect(),
            style: *style,
        },
        ExprNode::Hash { entries, braced } => ExprNode::Hash {
            entries: entries
                .iter()
                .map(|(k, v)| (rewrite_ivars_to_locals(k), rewrite_ivars_to_locals(v)))
                .collect(),
            braced: *braced,
        },
        ExprNode::Lambda { params, block_param, body, block_style } => ExprNode::Lambda {
            params: params.clone(),
            block_param: block_param.clone(),
            body: rewrite_ivars_to_locals(body),
            block_style: *block_style,
        },
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|p| match p {
                    InterpPart::Text { value } => InterpPart::Text { value: value.clone() },
                    InterpPart::Expr { expr } => InterpPart::Expr {
                        expr: rewrite_ivars_to_locals(expr),
                    },
                })
                .collect(),
        },
        other => other.clone(),
    };
    Expr::new(expr.span, new_node)
}

// ── predicate cond rewrite ───────────────────────────────────────

/// Rewrite Rails-style emptiness predicates to spinel-shape boolean
/// forms. Applied to the cond of every template-level `if`:
///   `recv.present?` / `recv.any?`  →  `!recv.empty?`
///   `recv.blank?`   / `recv.empty?` / `recv.none?`  →  `recv.empty?`
/// Recursive through `BoolOp` so `a.present? && b.any?` rewrites both
/// sides; other shapes pass through unchanged. Note this does NOT
/// generate the `!recv.nil? && !recv.empty?` nil-safe form — that
/// requires receiver-nullability info from the analyzer that the
/// lowerer doesn't have today (tracked as a follow-on slice).
fn rewrite_predicates(cond: &Expr) -> Expr {
    let new_node = match &*cond.node {
        ExprNode::Send {
            recv: Some(r),
            method,
            args,
            block: None,
            ..
        } if args.is_empty() => {
            let rewritten_recv = rewrite_predicates(r);
            match method.as_str() {
                "present?" | "any?" => {
                    // Unary `!`: emit as `Send { recv: None, method:
                    // "!", args: [empty_call] }` so the Ruby emitter
                    // produces `! recv.empty?` instead of the
                    // `recv.empty?.!` method-call form.
                    let empty_call = send(
                        Some(rewritten_recv),
                        "empty?",
                        Vec::new(),
                        None,
                        false,
                    );
                    return send(None, "!", vec![empty_call], None, false);
                }
                "blank?" | "empty?" | "none?" => {
                    return send(
                        Some(rewritten_recv),
                        "empty?",
                        Vec::new(),
                        None,
                        false,
                    );
                }
                _ => ExprNode::Send {
                    recv: Some(rewritten_recv),
                    method: method.clone(),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            }
        }
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_predicates(left),
            right: rewrite_predicates(right),
        },
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(rewrite_predicates),
            method: method.clone(),
            args: args.iter().map(rewrite_predicates).collect(),
            block: block.as_ref().map(rewrite_predicates),
            parenthesized: *parenthesized,
        },
        other => other.clone(),
    };
    Expr::new(cond.span, new_node)
}

fn rewrite_lvalue(lv: &LValue) -> LValue {
    match lv {
        LValue::Var { id, name } => LValue::Var { id: *id, name: name.clone() },
        LValue::Ivar { name } => LValue::Var { id: VarId(0), name: name.clone() },
        LValue::Attr { recv, name } => LValue::Attr {
            recv: rewrite_ivars_to_locals(recv),
            name: name.clone(),
        },
        LValue::Index { recv, index } => LValue::Index {
            recv: rewrite_ivars_to_locals(recv),
            index: rewrite_ivars_to_locals(index),
        },
    }
}

// ── extra-param collection ───────────────────────────────────────

/// Walk the (already ivar-rewritten) body and collect bareword
/// references — `Send { recv: None, args: [], block: None }` and
/// `Var` reads — whose names are NOT the inferred view arg, NOT
/// `_buf`, and NOT a recognized view helper. Today this catches
/// `notice` / `alert` (Rails flash helpers parsed as bare Sends). They
/// surface as positional params on the emitted method so the body
/// type-checks under spinel-blog's runtime.
fn collect_extra_params(body: &Expr, arg_name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut bound: Vec<String> = Vec::new();
    if !arg_name.is_empty() {
        bound.push(arg_name.to_string());
    }
    bound.push("_buf".to_string());
    bound.push("io".to_string());
    walk_for_extra(body, &bound, &mut out);
    out
}

fn walk_for_extra(e: &Expr, bound: &[String], out: &mut Vec<String>) {
    match &*e.node {
        ExprNode::Var { name, .. } => {
            let n = name.as_str();
            if !bound.iter().any(|b| b == n) && !out.iter().any(|x| x == n) && is_flash_name(n) {
                out.push(n.to_string());
            }
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            if recv.is_none() && args.is_empty() && block.is_none() {
                let n = method.as_str();
                if !bound.iter().any(|b| b == n)
                    && !out.iter().any(|x| x == n)
                    && is_flash_name(n)
                {
                    out.push(n.to_string());
                }
            }
            if let Some(r) = recv {
                walk_for_extra(r, bound, out);
            }
            for a in args {
                walk_for_extra(a, bound, out);
            }
            if let Some(b) = block {
                walk_for_extra(b, bound, out);
            }
        }
        ExprNode::Seq { exprs } => {
            for e in exprs {
                walk_for_extra(e, bound, out);
            }
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            walk_for_extra(cond, bound, out);
            walk_for_extra(then_branch, bound, out);
            walk_for_extra(else_branch, bound, out);
        }
        ExprNode::BoolOp { left, right, .. } => {
            walk_for_extra(left, bound, out);
            walk_for_extra(right, bound, out);
        }
        ExprNode::Assign { value, .. } => walk_for_extra(value, bound, out),
        ExprNode::Lambda { body, params, .. } => {
            let mut inner_bound = bound.to_vec();
            for p in params {
                inner_bound.push(p.as_str().to_string());
            }
            walk_for_extra(body, &inner_bound, out);
        }
        ExprNode::Array { elements, .. } => {
            for el in elements {
                walk_for_extra(el, bound, out);
            }
        }
        ExprNode::Hash { entries, .. } => {
            for (k, v) in entries {
                walk_for_extra(k, bound, out);
                walk_for_extra(v, bound, out);
            }
        }
        ExprNode::StringInterp { parts } => {
            for p in parts {
                if let InterpPart::Expr { expr } = p {
                    walk_for_extra(expr, bound, out);
                }
            }
        }
        _ => {}
    }
}

/// Today's heuristic for "this bareword is a Rails flash helper that
/// should surface as a method parameter." Conservative — any other
/// unknown bareword stays as a free reference and the analyzer / type
/// checker is responsible for diagnosing it. Expand the set as
/// fixtures introduce more flash-style helpers.
fn is_flash_name(n: &str) -> bool {
    matches!(n, "notice" | "alert")
}

// ── walker ───────────────────────────────────────────────────────

#[derive(Clone)]
#[allow(dead_code)] // arg_name + resource_dir read in follow-on slices.
struct ViewCtx {
    locals: Vec<String>,
    arg_name: String,
    resource_dir: String,
    /// Name of the local that accumulates output via `<<`. The
    /// top-level method body uses `io`; inside `form_with do |form|
    /// … end` blocks (and other capture-style helpers) the inner
    /// walk uses a fresh `body` so the captured string can be
    /// returned to the wrapping helper. Threaded through walk_body
    /// → walk_stmt → emit_io_append so every accumulator append
    /// resolves to the right local.
    accumulator: String,
    /// FormBuilder bindings active at this scope: `(local_name,
    /// record_name)` pairs. Populated when entering a `form_with`
    /// block; consumed by the FormBuilder method dispatch so
    /// `form.text_field :title` resolves to the bound record's
    /// model. Cleared on block exit.
    form_records: Vec<(String, String)>,
}

impl ViewCtx {
    fn is_local(&self, n: &str) -> bool {
        self.locals.iter().any(|x| x == n)
    }
    fn with_locals(&self, more: impl IntoIterator<Item = String>) -> Self {
        let mut next = self.clone();
        for n in more {
            if !next.locals.iter().any(|x| x == &n) {
                next.locals.push(n);
            }
        }
        next
    }
}

/// Walk a compiled-ERB body (`Seq` of `_buf = …` statements + control-
/// flow) and produce the corresponding spinel-shape statement list:
/// `io << ...` / `if cond ... end` / `coll.each { |x| ... }` / bare
/// helper-call statements (content_for setter), in source order.
fn walk_body(body: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    let stmts: Vec<&Expr> = match &*body.node {
        ExprNode::Seq { exprs } => exprs.iter().collect(),
        _ => vec![body],
    };
    let mut out = Vec::new();
    for stmt in &stmts {
        out.extend(walk_stmt(stmt, ctx));
    }
    out
}

fn walk_stmt(stmt: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    match &*stmt.node {
        // Prologue `_buf = ""` — drop; we inject `io = String.new` once
        // at method-body construction time.
        ExprNode::Assign { target: LValue::Var { name, .. }, value }
            if name.as_str() == "_buf" =>
        {
            if let ExprNode::Lit { value: Literal::Str { value: s } } = &*value.node {
                if s.is_empty() {
                    return Vec::new();
                }
            }
            // `_buf = _buf + X` — the working shape.
            if let ExprNode::Send { recv: Some(recv), method, args, .. } = &*value.node {
                if method.as_str() == "+" && args.len() == 1 {
                    if let ExprNode::Var { name: rn, .. } = &*recv.node {
                        if rn.as_str() == "_buf" {
                            return emit_io_append(&args[0], ctx);
                        }
                    }
                }
            }
            // Unrecognized `_buf = …` shape — emit as TODO comment-style
            // io append so the file still parses; the test asserts on
            // recognized shapes only.
            vec![todo_io_append("unknown _buf shape")]
        }
        // Epilogue `_buf` read — drop; the explicit trailing `io` is
        // appended once at method-body construction.
        ExprNode::Var { name, .. } if name.as_str() == "_buf" => Vec::new(),
        // Conditional branching at the template level. Cond goes
        // through `rewrite_predicates` so Rails-style `.present?` /
        // `.any?` / `.none?` / `.blank?` collapse to the
        // `.empty?`-based forms spinel's runtime expects.
        ExprNode::If { cond, then_branch, else_branch } => {
            let then_seq = walk_body(then_branch, ctx);
            let then_body = if then_seq.len() == 1 {
                then_seq.into_iter().next().unwrap()
            } else {
                seq(then_seq)
            };
            let else_body = if matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            ) {
                nil_lit()
            } else {
                let s = walk_body(else_branch, ctx);
                if s.len() == 1 {
                    s.into_iter().next().unwrap()
                } else {
                    seq(s)
                }
            };
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: rewrite_predicates(cond),
                    then_branch: then_body,
                    else_branch: else_body,
                },
            )]
        }
        // Statement-form view-helper calls. Today: `<% content_for
        // :title, "Articles" %>` lowers to `ViewHelpers.content_for_set
        // (:title, "Articles")` — a bare call, not appended to `io`.
        // Other recognized statement-form helpers fall through to a
        // TODO append until a fixture exercises them.
        ExprNode::Send { recv: None, method, args, block: None, .. } => {
            if let Some(ViewHelperKind::ContentForSetter { slot, body }) =
                classify_view_helper(method.as_str(), args)
            {
                return vec![view_helpers_call(
                    "content_for_set",
                    vec![lit_sym(Symbol::from(slot)), body.clone()],
                )];
            }
            vec![todo_io_append("unknown stmt")]
        }
        // Block-form `<% coll.each do |x| %>...<% end %>` at the
        // template level (rare — usually the each is inside a `<%= %>`
        // wrapper for collection partial render). When it shows up, we
        // recurse on the block body so inner `_buf = _buf + …` lines
        // become `io << …` against the outer io.
        ExprNode::Send {
            recv: Some(recv),
            method,
            args,
            block: Some(block),
            ..
        } if method.as_str() == "each" && args.is_empty() => {
            let ExprNode::Lambda { params, body, block_style, .. } = &*block.node else {
                return vec![todo_io_append("each block shape")];
            };
            let var_name = params
                .first()
                .map(|p| p.as_str().to_string())
                .unwrap_or_else(|| "item".into());
            let inner_ctx = ctx.with_locals([var_name.clone()]);
            let inner_stmts = walk_body(body, &inner_ctx);
            let inner_body = if inner_stmts.len() == 1 {
                inner_stmts.into_iter().next().unwrap()
            } else {
                seq(inner_stmts)
            };
            let block_lambda = Expr::new(
                Span::synthetic(),
                ExprNode::Lambda {
                    params: params.clone(),
                    block_param: None,
                    body: inner_body,
                    block_style: *block_style,
                },
            );
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Send {
                    recv: Some(recv.clone()),
                    method: method.clone(),
                    args: Vec::new(),
                    block: Some(block_lambda),
                    parenthesized: false,
                },
            )]
        }
        _ => vec![todo_io_append("unknown stmt")],
    }
}

/// Emit the IR for `io << <argument>` given the argument expression
/// from a `_buf = _buf + ARG` step. Splits into text-chunk vs.
/// output-interpolation handling — the latter goes through the
/// helper / partial / auto-escape classifiers.
fn emit_io_append(arg: &Expr, ctx: &ViewCtx) -> Vec<Expr> {
    // Text chunk → io << "literal".
    if let ExprNode::Lit { value: Literal::Str { .. } } = &*arg.node {
        return vec![accumulator_append_call(arg.clone(), ctx)];
    }
    // The compiler wraps `<%= expr %>` as `(expr).to_s`; strip that
    // wrapper. If the source wrote `<%= x.to_s %>` explicitly, we lose
    // the trailing `.to_s` — the round-trip is stable on the second
    // pass either way (matches the existing reconstruct_erb policy).
    let inner = unwrap_to_s(arg);

    // `<%= yield %>` and `<%= yield :slot %>` — appears in layouts
    // (and other capture-style templates that delegate body
    // rendering). Bare `yield` resolves to the layout's `body`
    // parameter (the rendered inner-view string); `yield :slot` is
    // a slot lookup against the content_for store.
    if let ExprNode::Yield { args: ya } = &*inner.node {
        return vec![accumulator_append_call(emit_yield(ya, ctx), ctx)];
    }

    // form_with capture: `<%= form_with(opts) do |form| ...inner... %>`
    // — wraps a sub-template. Lowers to a `ViewHelpers.form_with(opts)
    // do |form| body = String.new; <walked inner>; body end` call,
    // appended to the outer accumulator. The inner walk uses a fresh
    // `body` accumulator so the captured string is the form contents
    // (not concatenated to the outer io).
    if let ExprNode::Send {
        recv: None,
        method,
        args: sa,
        block: Some(block),
        ..
    } = &*inner.node
    {
        if method.as_str() == "form_with" {
            return vec![emit_form_with_capture(sa, block, ctx)];
        }
    }

    // FormBuilder method dispatch: `<%= form.text_field :title, opts
    // %>` where `form` is a known FormBuilder local. Pass through as
    // `<accumulator> << form.<method>(args)` (raw — FormBuilder output
    // is html-safe). `textarea` aliases to `text_area`; `submit` with
    // no positional gets a leading `nil`. Class-array opts simplify
    // to the base string entry.
    if let ExprNode::Send {
        recv: Some(r),
        method,
        args: sa,
        block: None,
        ..
    } = &*inner.node
    {
        if let ExprNode::Var { name, .. } = &*r.node {
            if ctx.form_records.iter().any(|(n, _)| n == name.as_str()) {
                if let Some(fb) = classify_form_builder_method(method.as_str()) {
                    let call = emit_form_builder_call(name.clone(), fb, sa);
                    return vec![accumulator_append_call(call, ctx)];
                }
            }
        }
    }

    // Render-partial classifier: `render @articles` / `render
    // @article.comments` / `render "x", k: v` → spinel-shape iteration
    // or named-partial dispatch. Wins over the helper classifier
    // because `render` is reserved.
    if let ExprNode::Send { recv, method, args: sa, block, .. } = &*inner.node {
        let is_local = |n: &str| ctx.is_local(n);
        if let Some(rp) =
            classify_render_partial(recv.as_ref(), method.as_str(), sa, block.as_ref(), &is_local)
        {
            if let Some(stmt) = emit_render_partial(&rp, ctx) {
                return vec![stmt];
            }
        }
    }

    // View-helper classifier: `link_to`, `dom_id`, `pluralize`,
    // `truncate`, `turbo_stream_from`, `content_for(:slot)`, …. The
    // classifier matches bare Sends (no recv, no block) only.
    if let ExprNode::Send { recv: None, method, args: sa, block: None, .. } = &*inner.node {
        if let Some(kind) = classify_view_helper(method.as_str(), sa) {
            if let Some(call) = emit_view_helper_call(&kind, ctx) {
                return vec![accumulator_append_call(call, ctx)];
            }
        }
    }

    // Default: bare interpolation `<%= expr %>` of a non-helper —
    // auto-escape. `<%= article.title %>` becomes
    // `io << ViewHelpers.html_escape(article.title)`. This matches
    // Rails's default behavior on `<%= %>` outside of helper output.
    //
    // Before wrapping, recurse through the expression and rewrite
    // any nested helper Sends to their ViewHelpers.* form so shapes
    // like `<%= content_for(:title) || "Real Blog" %>` (a BoolOp
    // whose left side is a bare helper call) come out as
    // `html_escape(ViewHelpers.content_for_get(:title) || "Real
    // Blog")` rather than carrying the raw `content_for` Send.
    let rewritten = rewrite_helpers_in_expr(inner, ctx);
    let escaped = view_helpers_call("html_escape", vec![rewritten]);
    vec![accumulator_append_call(escaped, ctx)]
}

/// Recursively walk `expr` and rewrite any bare view-helper Send
/// (`Send { recv: None, method, args, block: None }`) into its
/// `ViewHelpers.*` form via `classify_view_helper` +
/// `emit_view_helper_call`. Threads through `BoolOp` and `Send`
/// children so helpers nested inside expressions get reached. Other
/// shapes pass through; leaf nodes are unchanged.
///
/// The auto-escape fallback uses this so `<%= helper(...) || default
/// %>` and similar combinations end up with the helper rewritten
/// before the html_escape wrap, matching the convention every
/// emitter would otherwise have to repeat.
fn rewrite_helpers_in_expr(e: &Expr, ctx: &ViewCtx) -> Expr {
    let new_node = match &*e.node {
        ExprNode::Send {
            recv: None,
            method,
            args,
            block: None,
            parenthesized,
        } => {
            if let Some(kind) = classify_view_helper(method.as_str(), args) {
                if let Some(call) = emit_view_helper_call(&kind, ctx) {
                    return call;
                }
            }
            ExprNode::Send {
                recv: None,
                method: method.clone(),
                args: args.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect(),
                block: None,
                parenthesized: *parenthesized,
            }
        }
        ExprNode::Send { recv, method, args, block, parenthesized } => ExprNode::Send {
            recv: recv.as_ref().map(|r| rewrite_helpers_in_expr(r, ctx)),
            method: method.clone(),
            args: args.iter().map(|a| rewrite_helpers_in_expr(a, ctx)).collect(),
            block: block.as_ref().map(|b| rewrite_helpers_in_expr(b, ctx)),
            parenthesized: *parenthesized,
        },
        ExprNode::BoolOp { op, surface, left, right } => ExprNode::BoolOp {
            op: *op,
            surface: *surface,
            left: rewrite_helpers_in_expr(left, ctx),
            right: rewrite_helpers_in_expr(right, ctx),
        },
        other => other.clone(),
    };
    Expr::new(e.span, new_node)
}

fn unwrap_to_s(expr: &Expr) -> &Expr {
    if let ExprNode::Send { recv: Some(inner), method, args, .. } = &*expr.node {
        if method.as_str() == "to_s" && args.is_empty() {
            return inner;
        }
    }
    expr
}

// ── view-helper call construction ────────────────────────────────

fn emit_view_helper_call(kind: &ViewHelperKind<'_>, ctx: &ViewCtx) -> Option<Expr> {
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
        JavascriptImportmapTags => {
            Some(view_helpers_call("javascript_importmap_tags", Vec::new()))
        }
        // `<%= stylesheet_link_tag :app, "data-turbo-track":
        // "reload" %>` — first arg is the stylesheet group; spinel-
        // blog converts the `:app` symbol form to the `"app"` string
        // form (the runtime expects the string key). Trailing opts
        // hash threads through unchanged; non-symbol keys
        // (`"data-turbo-track":`) emit in rocket form via the Ruby
        // emitter's hash printer.
        StylesheetLinkTag { name, opts } => {
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

// ── render-partial dispatch ──────────────────────────────────────

fn emit_render_partial(rp: &RenderPartial<'_>, ctx: &ViewCtx) -> Option<Expr> {
    match rp {
        // `render articles` (collection) — iterate, rendering one
        // partial per element. The partial-fn name comes from the
        // singular form of the local: `articles` → `Views::Articles
        // .article(a)`. The inner `io << ...` uses the active
        // accumulator so a `render @articles` inside a form_with
        // capture appends to `body` rather than the outer `io`.
        RenderPartial::Collection { name, .. } => {
            let plural = *name;
            let collection_recv = var_ref(Symbol::from(plural));
            Some(emit_partial_each(&collection_recv, plural, ctx))
        }
        // `render "form", article: @article` — explicit-name partial.
        // Rewrites to `<accumulator> << Views::<Plural>.<method>(arg)`
        // — a single Send append, NOT an each block, since named
        // partials render once. The partial path can be a slash-form
        // (`"comments/comment"`) routing to a different module; bare
        // names (`"form"`) resolve to the current resource_dir's
        // module. The arg is the first hash entry's value (Rails
        // convention; additional hash entries get dropped today —
        // matches existing classifier policy).
        RenderPartial::Named { partial, arg } => {
            let (module_dir, base_name) = match partial.rsplit_once('/') {
                Some((dir, name)) => (dir.to_string(), name.to_string()),
                None => (ctx.resource_dir.clone(), (*partial).to_string()),
            };
            if module_dir.is_empty() {
                return None;
            }
            let module_camel = camelize(&snake_case(&module_dir));
            let method_sym = base_name.trim_start_matches('_').to_string();

            let arg_expr = arg.cloned().unwrap_or_else(nil_lit);
            let render_call = send(
                Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Const {
                        path: vec![Symbol::from("Views"), Symbol::from(module_camel)],
                    },
                )),
                &method_sym,
                vec![arg_expr],
                None,
                true,
            );
            Some(accumulator_append_call(render_call, ctx))
        }
        // `render @article.comments` — has_many association iteration.
        // The `receiver` is the post-ivar-rewrite `Var(article)` (or a
        // bareword `Send`); `method` is the assoc name, plural. We
        // build `receiver.method.each { |c| io << Views::<Plural>
        // .<singular>(c) }`. No has_many table lookup needed: the
        // method dispatch on `receiver` resolves to whatever the
        // model's lowered association method returns at runtime.
        RenderPartial::Association { receiver, method } => {
            let assoc_recv = send(
                Some((*receiver).clone()),
                method,
                Vec::new(),
                None,
                false,
            );
            Some(emit_partial_each(&assoc_recv, method, ctx))
        }
    }
}

/// Common shape for collection / association partial renders:
/// `<recv>.each { |x| <accumulator> << Views::<Plural>.<singular>(x) }`.
/// `plural_name` is the resource name in plural form
/// (`articles` / `comments`); the partial-fn name is its singular
/// (`article` / `comment`); the variable name is the singular's first
/// letter (`a` / `c`).
fn emit_partial_each(recv: &Expr, plural_name: &str, ctx: &ViewCtx) -> Expr {
    let singular = singularize(plural_name);
    let plural_camel = camelize(&snake_case(plural_name));
    let var_name = Symbol::from(singular.chars().next().unwrap_or('x').to_string());

    let render_call = send(
        Some(Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![Symbol::from("Views"), Symbol::from(plural_camel)],
            },
        )),
        &singular,
        vec![var_ref(var_name.clone())],
        None,
        true,
    );
    let inner = accumulator_append_call(render_call, ctx);
    let block_lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![var_name],
            block_param: None,
            body: inner,
            block_style: BlockStyle::Brace,
        },
    );
    send(
        Some(recv.clone()),
        "each",
        Vec::new(),
        Some(block_lambda),
        false,
    )
}

// ── yield ────────────────────────────────────────────────────────

/// Lower a Yield expr the layout body can produce:
///   `<%= yield %>`        →  `body` (the layout's body parameter)
///   `<%= yield :slot %>`  →  `ViewHelpers.get_slot(:slot)`
/// Bare yield uses the inferred view arg (`body` for layouts).
/// Outside of layouts, bare yield is malformed Rails ERB anyway —
/// we still emit a Var read against whatever the inferred arg was.
fn emit_yield(args: &[Expr], ctx: &ViewCtx) -> Expr {
    if let Some(first) = args.first() {
        if let ExprNode::Lit { value: Literal::Sym { value } } = &*first.node {
            return view_helpers_call("get_slot", vec![lit_sym(value.clone())]);
        }
    }
    let local_name = if ctx.arg_name.is_empty() {
        "body".to_string()
    } else {
        ctx.arg_name.clone()
    };
    var_ref(Symbol::from(local_name))
}

// ── form_with capture ────────────────────────────────────────────

/// Lower `<%= form_with(opts) do |form| ...inner... %>` into
/// `<accumulator> << ViewHelpers.form_with(opts) do |form|
///     body = String.new ; <walked inner> ; body
/// end`. The block body is itself a compiled-ERB template; we
/// recursively walk it with a fresh `body` accumulator so the
/// inner `_buf = _buf + …` lines become `body << …` and the block
/// returns the captured string.
///
/// Today the original `opts` hash passes through unchanged (today
/// this means the lowered call carries the surface kwargs the
/// developer wrote — `model:`, `class:`, etc. — rather than the
/// spinel-blog's restructured `model: / model_name: / action: /
/// method: / opts:` form). The action/method computation from
/// `record.persisted?` is a follow-on slice; this slice's job is
/// the capture mechanism + FormBuilder dispatch wiring.
fn emit_form_with_capture(args: &[Expr], block: &Expr, ctx: &ViewCtx) -> Expr {
    let ExprNode::Lambda { params, body, block_style, .. } = &*block.node else {
        return accumulator_append_call(lit_str(String::new()), ctx);
    };
    let form_param = params
        .first()
        .cloned()
        .unwrap_or_else(|| Symbol::from("form"));

    // Resolve `model:` kwarg → record local name. The FormBuilder
    // dispatch needs this so `form.text_field :title` can look up
    // the record's attribute when emitting (a future slice; today
    // the local name + form_param binding alone is enough).
    let record_local = find_kwarg_local_name(args);

    let mut inner_ctx = ctx.with_locals([form_param.as_str().to_string()]);
    inner_ctx.accumulator = "body".to_string();
    // Register the form param unconditionally — the FormBuilder
    // dispatch matches on the local NAME, not the record. The
    // record-name metadata gets used by attribute-aware lowerings
    // (future slice); a complex model expression like
    // `[article, Comment.new]` leaves it empty.
    inner_ctx.form_records.push((
        form_param.as_str().to_string(),
        record_local.unwrap_or_default(),
    ));

    // Walk the inner template with the fresh accumulator. Wrap the
    // walked stmts with the per-block prologue (`body = String.new`)
    // and epilogue (trailing `body` for the block return value).
    let mut block_body_stmts: Vec<Expr> = Vec::new();
    block_body_stmts.push(assign_accumulator_string_new(&inner_ctx.accumulator));
    block_body_stmts.extend(walk_body(body, &inner_ctx));
    block_body_stmts.push(var_ref(Symbol::from(inner_ctx.accumulator.as_str())));
    let inner_seq = seq(block_body_stmts);

    let block_lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![form_param],
            block_param: None,
            body: inner_seq,
            block_style: *block_style,
        },
    );

    let form_with_call = view_helpers_call_with_block("form_with", args.to_vec(), block_lambda);
    accumulator_append_call(form_with_call, ctx)
}

/// `ViewHelpers.<method>(args) do |params| body end` — companion to
/// `view_helpers_call` that takes an attached block. Used for
/// capture-style helpers (`form_with`, `content_for` block-form).
fn view_helpers_call_with_block(method: &str, args: Vec<Expr>, block: Expr) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("ViewHelpers")] },
    );
    send(Some(recv), method, args, Some(block), true)
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

// ── FormBuilder method dispatch ──────────────────────────────────

/// Emit a FormBuilder call: `form.<method>(positional, opts)`.
/// Method-name remapping: the Rails `textarea` alias normalizes to
/// `text_area` (spinel-blog's runtime exposes the underscore form
/// only). `submit` with no positional arg gets a leading `nil` —
/// matches spinel-blog's `form.submit(nil, class: "...")` shape.
/// Trailing opts hash, if present, runs through
/// `simplify_class_array` so `class: ["base", {…}]` collapses to
/// `class: "base"` (the conditional clauses drop today; an
/// errors-aware composition lands when a fixture forces it).
fn emit_form_builder_call(recv_name: Symbol, kind: FormBuilderMethod, args: &[Expr]) -> Expr {
    let method_name = match kind {
        FormBuilderMethod::Label => "label",
        FormBuilderMethod::TextField => "text_field",
        FormBuilderMethod::TextArea => "text_area",
        FormBuilderMethod::Submit => "submit",
    };
    let mut new_args: Vec<Expr> = args.iter().map(simplify_arg_class_array).collect();
    if matches!(kind, FormBuilderMethod::Submit) {
        // `form.submit class: "..."` had no positional in the source;
        // spinel runtime expects `form.submit(label, opts)`. Insert
        // a leading nil when the first arg isn't a positional value.
        let first_is_hash = matches!(
            new_args.first().map(|a| &*a.node),
            Some(ExprNode::Hash { .. }),
        );
        if new_args.is_empty() || first_is_hash {
            new_args.insert(0, nil_lit());
        }
    }
    send(Some(var_ref(recv_name)), method_name, new_args, None, true)
}

/// Walk one positional/opts arg and simplify a `class:` Hash entry
/// whose value is a Rails-style `["base", {cond_class: pred, …}]`
/// array. Replaces the array with just the base string. Other entries
/// pass through unchanged.
fn simplify_arg_class_array(arg: &Expr) -> Expr {
    let ExprNode::Hash { entries, braced } = &*arg.node else {
        return arg.clone();
    };
    let new_entries: Vec<(Expr, Expr)> = entries
        .iter()
        .map(|(k, v)| {
            let is_class_key = matches!(
                &*k.node,
                ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "class",
            );
            if is_class_key {
                (k.clone(), simplify_class_array(v))
            } else {
                (k.clone(), v.clone())
            }
        })
        .collect();
    Expr::new(
        arg.span,
        ExprNode::Hash { entries: new_entries, braced: *braced },
    )
}

/// `["base_string", {cond_class: pred, …}]` → `"base_string"`.
/// Anything else passes through unchanged.
fn simplify_class_array(v: &Expr) -> Expr {
    let ExprNode::Array { elements, .. } = &*v.node else {
        return v.clone();
    };
    let Some(first) = elements.first() else {
        return v.clone();
    };
    if matches!(&*first.node, ExprNode::Lit { value: Literal::Str { .. } }) {
        return first.clone();
    }
    v.clone()
}

// ── small IR constructors ────────────────────────────────────────

/// `<accumulator> = String.new` — synthesized once per template body.
/// The accumulator name comes from the active ViewCtx (`io` at top
/// level; `body` inside `form_with` blocks).
fn assign_accumulator_string_new(name: &str) -> Expr {
    let string_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("String")] },
    );
    let new_call = send(Some(string_const), "new", Vec::new(), None, false);
    Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: Symbol::from(name) },
            value: new_call,
        },
    )
}

/// `<accumulator> << <arg>` — the per-step append. Always emits with
/// `<<` (a binary operator the Ruby emit_send_base rewrites to infix
/// form), so the source comes out as `io << arg`, not `io.<<(arg)`.
fn accumulator_append_call(arg: Expr, ctx: &ViewCtx) -> Expr {
    send(
        Some(var_ref(Symbol::from(ctx.accumulator.as_str()))),
        "<<",
        vec![arg],
        None,
        false,
    )
}

fn view_helpers_call(method: &str, args: Vec<Expr>) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("ViewHelpers")] },
    );
    send(Some(recv), method, args, None, true)
}

fn route_helpers_call(method: &str, args: Vec<Expr>) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
    );
    send(Some(recv), method, args, None, true)
}

fn inflector_call(method: &str, args: Vec<Expr>) -> Expr {
    let recv = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from("Inflector")] },
    );
    send(Some(recv), method, args, None, true)
}

/// A `Send` constructor that makes the parenthesized flag explicit on
/// the call site. The Ruby emitter ignores the flag for zero-arg calls
/// (always emits `recv.method`), so it's safe to pass `true` for any
/// helper Send regardless of arity.
fn send(
    recv: Option<Expr>,
    method: &str,
    args: Vec<Expr>,
    block: Option<Expr>,
    parenthesized: bool,
) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(method),
            args,
            block,
            parenthesized,
        },
    )
}

fn lit_str(s: String) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value: s } },
    )
}

fn lit_sym(s: Symbol) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Sym { value: s } },
    )
}

fn nil_lit() -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil })
}

fn var_ref(name: Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Var { id: VarId(0), name })
}

fn seq(exprs: Vec<Expr>) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Seq { exprs })
}

/// Placeholder for unrecognized template shapes — keeps the lowered
/// output well-formed Ruby (a no-op string append) so the file parses.
/// The tag is purely advisory; callers can grep for it to find gaps.
/// The accumulator-aware path uses `walk_stmt`'s ctx, but this helper
/// has none in scope, so it falls back to the default `io` accumulator.
/// Acceptable since today's gaps either land at the top level or
/// inside scopes that still have an `io` shadow at runtime.
fn todo_io_append(tag: &str) -> Expr {
    let _ = tag;
    send(
        Some(var_ref(Symbol::from("io"))),
        "<<",
        vec![lit_str(String::new())],
        None,
        false,
    )
}

// ── tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_id_for_articles_dir() {
        let id = view_module_id("articles");
        assert_eq!(id.0.as_str(), "Views::Articles");
    }

    #[test]
    fn arg_name_index_is_plural() {
        let n = infer_view_arg("index", "articles", false, &[]);
        assert_eq!(n, "articles");
    }

    #[test]
    fn arg_name_partial_is_singular() {
        let n = infer_view_arg("article", "articles", true, &[]);
        assert_eq!(n, "article");
    }

    #[test]
    fn arg_name_show_is_singular() {
        let n = infer_view_arg("show", "articles", false, &[]);
        assert_eq!(n, "article");
    }
}
