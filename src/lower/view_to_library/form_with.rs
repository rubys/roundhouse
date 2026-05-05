//! `form_with` capture lowering: turn `<%= form_with(opts) do |form|
//! ...inner... %>` into a `ViewHelpers.form_with(restructured) do
//! |form| body = String.new ; <walked inner> ; body end` call.

use crate::expr::{Expr, ExprNode, InterpPart, Literal};
use crate::ident::Symbol;
use crate::naming::{singularize, snake_case};
use crate::span::Span;

use super::walker::walk_body;
use super::{
    accumulator_append_call, assign_accumulator_string_new, lit_str, lit_sym, send, var_ref,
    ViewCtx,
};

/// Lower `<%= form_with(opts) do |form| ...inner... %>` into
/// `<accumulator> << ViewHelpers.form_with(opts) do |form|
///     body = String.new ; <walked inner> ; body
/// end`. The block body is itself a compiled-ERB template; we
/// recursively walk it with a fresh `body` accumulator so the
/// inner `_buf = _buf + …` lines become `body << …` and the block
/// returns the captured string.
///
/// The surface call's kwargs are restructured into the spinel
/// runtime's required shape: `model:`, `model_name:` (singular of
/// `resource_dir`), `action:` and `method:` (computed from
/// `record.persisted?`), and `opts:` (any leftover surface kwargs).
/// See `restructure_form_with_kwargs`.
pub(super) fn emit_form_with_capture(args: &[Expr], block: &Expr, ctx: &ViewCtx) -> Expr {
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
    let inner_seq = super::seq(block_body_stmts);

    let block_lambda = Expr::new(
        Span::synthetic(),
        ExprNode::Lambda {
            params: vec![form_param],
            block_param: None,
            body: inner_seq,
            block_style: *block_style,
        },
    );

    let restructured = restructure_form_with_kwargs(args, ctx);
    let form_with_call = view_helpers_call_with_block("form_with", restructured, block_lambda);
    accumulator_append_call(form_with_call, ctx)
}

/// Map the surface `form_with(model: rec, class: "...")` kwargs to
/// the spinel runtime's expected shape:
///   `model: rec, model_name: "<singular>", action: <expr>, method: <sym>, opts: { class: "..." }`
/// where `action`/`method` branch on `rec.persisted?` so a new record
/// posts to the collection path and an existing record patches the
/// member path. `model_name` derives from `ctx.resource_dir`
/// (e.g. `"articles"` → `"article"`). Unknown kwargs collect into
/// `opts:` so the runtime can stringify them onto the `<form>` tag.
fn restructure_form_with_kwargs(args: &[Expr], ctx: &ViewCtx) -> Vec<Expr> {
    let mut model_expr: Option<Expr> = None;
    let mut opts_entries: Vec<(Expr, Expr)> = Vec::new();
    let mut other_args: Vec<Expr> = Vec::new();

    for arg in args {
        if let ExprNode::Hash { entries, .. } = &*arg.node {
            for (k, v) in entries {
                if let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node {
                    if key.as_str() == "model" {
                        model_expr = Some(v.clone());
                        continue;
                    }
                }
                opts_entries.push((k.clone(), v.clone()));
            }
        } else {
            other_args.push(arg.clone());
        }
    }

    let Some(model) = model_expr else {
        // No `model:` kwarg — pass through unchanged. Non-resource
        // form_with isn't exercised by the fixture; if it becomes
        // a real shape, derive `model_name`/`action` differently.
        return args.to_vec();
    };

    // Polymorphic array form: `model: [parent, child]` (nested resource).
    // Spinel-blog rewrites these as
    // `model: <child>, model_name: "<child_singular>",
    //  action: RouteHelpers.<parent_local>_<child_plural>_path(<parent>.id),
    //  method: :post` (the `Class.new` last element is never persisted).
    // Detected when `model:` is an Array literal whose last element is a
    // `Class.new(...)` Send. Other array shapes fall through.
    let route_helpers = || {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const { path: vec![Symbol::from("RouteHelpers")] },
        )
    };

    if let Some((nested_model, nested_name, nested_action)) =
        nested_resource_form(&model, &route_helpers)
    {
        let opts_hash = Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries: opts_entries, kwargs: false },
        );
        let entries: Vec<(Expr, Expr)> = vec![
            (lit_sym(Symbol::from("model")), nested_model),
            (lit_sym(Symbol::from("model_name")), lit_str(nested_name)),
            (lit_sym(Symbol::from("action")), nested_action),
            (lit_sym(Symbol::from("method")), lit_sym(Symbol::from("post"))),
            (lit_sym(Symbol::from("opts")), opts_hash),
        ];
        let new_hash = Expr::new(
            Span::synthetic(),
            ExprNode::Hash { entries, kwargs: true },
        );
        let mut out = other_args;
        out.push(new_hash);
        return out;
    }

    let plural = ctx.resource_dir.as_str();
    let singular = singularize(plural);

    let model_name = lit_str(singular.clone());

    // record.persisted?
    let persisted = send(Some(model.clone()), "persisted?", Vec::new(), None, false);

    // RouteHelpers.<singular>_path(record.id)
    let model_id = send(Some(model.clone()), "id", Vec::new(), None, false);
    let member_path = send(
        Some(route_helpers()),
        &format!("{singular}_path"),
        vec![model_id],
        None,
        true,
    );
    // RouteHelpers.<plural>_path
    let collection_path = send(
        Some(route_helpers()),
        &format!("{plural}_path"),
        Vec::new(),
        None,
        false,
    );

    let action = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: persisted.clone(),
            then_branch: member_path,
            else_branch: collection_path,
        },
    );
    let method = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: persisted,
            then_branch: lit_sym(Symbol::from("patch")),
            else_branch: lit_sym(Symbol::from("post")),
        },
    );

    let opts_hash = Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries: opts_entries, kwargs: false },
    );

    let new_entries: Vec<(Expr, Expr)> = vec![
        (lit_sym(Symbol::from("model")), model),
        (lit_sym(Symbol::from("model_name")), model_name),
        (lit_sym(Symbol::from("action")), action),
        (lit_sym(Symbol::from("method")), method),
        (lit_sym(Symbol::from("opts")), opts_hash),
    ];
    // `kwargs: true` so the kwargs render bare (`model: rec, …`) at
    // the call site, matching `f(a: 1, b: 2)` Ruby surface. The inner
    // `opts:` value above stays braced because it's an explicit hash.
    let new_hash = Expr::new(
        Span::synthetic(),
        ExprNode::Hash { entries: new_entries, kwargs: true },
    );

    let mut out = other_args;
    out.push(new_hash);
    out
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
