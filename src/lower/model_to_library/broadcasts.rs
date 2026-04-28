//! broadcasts_to expansion: one DSL line synthesizes three lifecycle
//! methods (after_create_commit / after_update_commit /
//! after_destroy_commit), each calling `Broadcasts.<action>(stream:,
//! target:, html:)`. The lambda-form channel's param (e.g. `comment`
//! in `->(comment) { "article_#{comment.article_id}_comments" }`)
//! gets rewritten to ivar / self references so the expanded body
//! reads from the model's own state.
//!
//! Convention (mirrors Rails turbo + spinel-blog reference):
//!   - create: action = inserts_by (default :append). target = explicit
//!     `target:` override OR the channel string (when literal). html =
//!     `Views::<Plural>.<singular>(self)`.
//!   - update: action = :replace. target = "<class_singular>_#{@id}".
//!     html = `Views::<Plural>.<singular>(self)`.
//!   - destroy: action = :remove. target = "<class_singular>_#{@id}".
//!     no html (remove takes no payload).

use crate::dialect::{Association, MethodDef, Model, ModelBodyItem};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;

use super::markers::fold_into_or_push;
use super::{lit_sym, nil_lit, self_ref, var_ref};

pub(super) fn push_broadcasts_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "broadcasts_to" {
            continue;
        }
        if args.is_empty() {
            continue;
        }

        let (channel_expr, self_param) = match &*args[0].node {
            ExprNode::Lambda { body, params, .. } => (body.clone(), params.first().cloned()),
            ExprNode::Lit { value: Literal::Str { .. } } => (args[0].clone(), None),
            _ => continue,
        };

        let mut create_action = BroadcastAct::Append;
        let mut create_target_override: Option<Expr> = None;
        if let Some(opts) = args.get(1) {
            if let ExprNode::Hash { entries, .. } = &*opts.node {
                for (k, v) in entries {
                    let Some(key) = sym_key(k) else { continue };
                    match key.as_str() {
                        "inserts_by" => {
                            if let ExprNode::Lit { value: Literal::Sym { value } } = &*v.node {
                                create_action = match value.as_str() {
                                    "prepend" => BroadcastAct::Prepend,
                                    "replace" => BroadcastAct::Replace,
                                    "append" => BroadcastAct::Append,
                                    _ => BroadcastAct::Append,
                                };
                            }
                        }
                        "target" => create_target_override = Some(v.clone()),
                        _ => {}
                    }
                }
            }
        }

        let stream_expr = rewrite_lambda_param(&channel_expr, self_param.as_ref());
        let create_target = create_target_override
            .map(|t| rewrite_lambda_param(&t, self_param.as_ref()))
            .unwrap_or_else(|| stream_expr.clone());
        let canonical_target = canonical_record_target(&model.name);
        let html_partial = views_render_self(&model.name);

        let create_call = broadcasts_call(
            create_action,
            stream_expr.clone(),
            create_target,
            Some(html_partial.clone()),
        );
        let update_call = broadcasts_call(
            BroadcastAct::Replace,
            stream_expr.clone(),
            canonical_target.clone(),
            Some(html_partial),
        );
        let destroy_call = broadcasts_call(
            BroadcastAct::Remove,
            stream_expr,
            canonical_target,
            None,
        );

        fold_into_or_push(methods, model, "after_create_commit", create_call);
        fold_into_or_push(methods, model, "after_update_commit", update_call);
        fold_into_or_push(methods, model, "after_destroy_commit", destroy_call);
    }
}

#[derive(Clone, Copy)]
enum BroadcastAct {
    Append,
    Prepend,
    Replace,
    Remove,
}

impl BroadcastAct {
    fn method_name(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Prepend => "prepend",
            Self::Replace => "replace",
            Self::Remove => "remove",
        }
    }
}

fn broadcasts_call(
    action: BroadcastAct,
    stream: Expr,
    target: Expr,
    html: Option<Expr>,
) -> Expr {
    let mut entries: Vec<(Expr, Expr)> = vec![
        (lit_sym(Symbol::from("stream")), stream),
        (lit_sym(Symbol::from("target")), target),
    ];
    if let Some(h) = html {
        entries.push((lit_sym(Symbol::from("html")), h));
    }
    let kwargs = Expr::new(Span::synthetic(), ExprNode::Hash { entries, braced: false });
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const { path: vec![Symbol::from("Broadcasts")] },
            )),
            method: Symbol::from(action.method_name()),
            args: vec![kwargs],
            block: None,
            parenthesized: true,
        },
    )
}

/// `"<class_singular>_#{@id}"` — the canonical per-record DOM target
/// Rails turbo uses on update + destroy regardless of `target:` option.
fn canonical_record_target(class_name: &ClassId) -> Expr {
    let singular = crate::naming::snake_case(class_name.0.as_str());
    Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp {
            parts: vec![
                crate::expr::InterpPart::Text { value: format!("{singular}_") },
                crate::expr::InterpPart::Expr {
                    expr: Expr::new(
                        Span::synthetic(),
                        ExprNode::Ivar { name: Symbol::from("id") },
                    ),
                },
            ],
        },
    )
}

/// `Views::<Plural>.<singular>(self)` — the partial-render call used
/// for the `html:` payload on create/update broadcasts.
fn views_render_self(class_name: &ClassId) -> Expr {
    let plural = crate::naming::pluralize_snake(class_name.0.as_str());
    let plural_camel = camelize(&plural);
    let singular = crate::naming::snake_case(class_name.0.as_str());
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const {
                    path: vec![Symbol::from("Views"), Symbol::from(plural_camel)],
                },
            )),
            method: Symbol::from(singular),
            args: vec![self_ref()],
            block: None,
            parenthesized: true,
        },
    )
}

fn camelize(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    let mut upper = true;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// Rewrite `param.attr` → `@attr` and bare `param` → `self`. The
/// channel/target lambda's parameter refers to the record being
/// broadcast; in the expanded method body those references resolve
/// to the model's own state.
fn rewrite_lambda_param(e: &Expr, param: Option<&Symbol>) -> Expr {
    let Some(p) = param else { return e.clone() };
    let new_node = match &*e.node {
        ExprNode::Var { name, .. } if name == p => ExprNode::SelfRef,
        ExprNode::Send { recv: Some(r), method, args, block, parenthesized } => {
            // `param.attr` (no args, no block) → `@attr`.
            if let ExprNode::Var { name, .. } = &*r.node {
                if name == p && args.is_empty() && block.is_none() {
                    return Expr::new(
                        Span::synthetic(),
                        ExprNode::Ivar { name: method.clone() },
                    );
                }
            }
            ExprNode::Send {
                recv: Some(rewrite_lambda_param(r, Some(p))),
                method: method.clone(),
                args: args.iter().map(|a| rewrite_lambda_param(a, Some(p))).collect(),
                block: block.as_ref().map(|b| rewrite_lambda_param(b, Some(p))),
                parenthesized: *parenthesized,
            }
        }
        ExprNode::StringInterp { parts } => ExprNode::StringInterp {
            parts: parts
                .iter()
                .map(|part| match part {
                    crate::expr::InterpPart::Text { value } => {
                        crate::expr::InterpPart::Text { value: value.clone() }
                    }
                    crate::expr::InterpPart::Expr { expr } => crate::expr::InterpPart::Expr {
                        expr: rewrite_lambda_param(expr, Some(p)),
                    },
                })
                .collect(),
        },
        _ => return e.clone(),
    };
    Expr::new(Span::synthetic(), new_node)
}

/// Rewrite Rails-API `<assoc>.broadcast_<action>_to(<stream>)` calls
/// (typical inside `after_<x>_commit { ... }` blocks) to spinel-shape:
///
///     parent = <assoc>
///     return if parent.nil?
///     Broadcasts.<action>(stream: <stream>,
///                         target: "<sing>_#{parent.id}",
///                         html: Views::<Plur>.<sing>(parent))
///
/// `<assoc>` must name a `belongs_to` association on `model` so the
/// target class — and from it the stream's per-record DOM target +
/// partial render — is resolvable. `<call> rescue nil` modifiers
/// strip away: the explicit `parent.nil?` early-return covers the
/// "association is missing" case the rescue was guarding against.
///
/// Other shapes (non-belongs_to receiver, unknown method) pass
/// through unchanged so the emitter still produces parseable Ruby.
pub(super) fn rewrite_rails_broadcast_calls(expr: Expr, model: &Model) -> Expr {
    walk(expr, model)
}

fn walk(e: Expr, model: &Model) -> Expr {
    // `<call> rescue nil` where `<call>` is a recognized broadcast →
    // unwrap the rescue (the spinel shape's nil-check supersedes).
    if let ExprNode::RescueModifier { expr: inner, fallback } = &*e.node {
        if matches!(&*fallback.node, ExprNode::Lit { value: Literal::Nil }) {
            if let Some(rewritten) = try_rewrite_call(inner, model) {
                return rewritten;
            }
        }
    }
    if let Some(rewritten) = try_rewrite_call(&e, model) {
        return rewritten;
    }
    let new_node = match &*e.node {
        ExprNode::Seq { exprs } => ExprNode::Seq {
            exprs: exprs.iter().map(|x| walk(x.clone(), model)).collect(),
        },
        ExprNode::If { cond, then_branch, else_branch } => ExprNode::If {
            cond: walk(cond.clone(), model),
            then_branch: walk(then_branch.clone(), model),
            else_branch: walk(else_branch.clone(), model),
        },
        ExprNode::RescueModifier { expr, fallback } => ExprNode::RescueModifier {
            expr: walk(expr.clone(), model),
            fallback: walk(fallback.clone(), model),
        },
        _ => return e,
    };
    Expr::new(e.span, new_node)
}

fn try_rewrite_call(expr: &Expr, model: &Model) -> Option<Expr> {
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block: None,
        ..
    } = &*expr.node
    else {
        return None;
    };
    let action = match method.as_str() {
        "broadcast_replace_to" => BroadcastAct::Replace,
        "broadcast_append_to" => BroadcastAct::Append,
        "broadcast_remove_to" => BroadcastAct::Remove,
        _ => return None,
    };
    let ExprNode::Send {
        recv: None,
        method: assoc_name,
        args: a_args,
        block: None,
        ..
    } = &*recv.node
    else {
        return None;
    };
    if !a_args.is_empty() {
        return None;
    }
    let target_class = model.associations().find_map(|a| match a {
        Association::BelongsTo { name, target, .. } if name == assoc_name => Some(target),
        _ => None,
    })?;
    let stream_arg = args.first().cloned()?;

    let class_name = target_class.0.as_str();
    let singular = crate::naming::snake_case(class_name);
    let plural = crate::naming::pluralize_snake(class_name);
    let plural_camel = camelize(&plural);

    let parent_sym = Symbol::from("parent");
    let assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: LValue::Var { id: VarId(0), name: parent_sym.clone() },
            value: recv.clone(),
        },
    );
    let nil_check = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(parent_sym.clone())),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let return_if = Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond: nil_check,
            then_branch: Expr::new(
                Span::synthetic(),
                ExprNode::Return { value: nil_lit() },
            ),
            else_branch: nil_lit(),
        },
    );
    let parent_id = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(parent_sym.clone())),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let target_str = Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp {
            parts: vec![
                crate::expr::InterpPart::Text { value: format!("{singular}_") },
                crate::expr::InterpPart::Expr { expr: parent_id },
            ],
        },
    );
    let views_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(Expr::new(
                Span::synthetic(),
                ExprNode::Const {
                    path: vec![Symbol::from("Views"), Symbol::from(plural_camel)],
                },
            )),
            method: Symbol::from(singular.clone()),
            args: vec![var_ref(parent_sym.clone())],
            block: None,
            parenthesized: true,
        },
    );

    let html = if matches!(action, BroadcastAct::Remove) {
        None
    } else {
        Some(views_call)
    };
    let broadcast_call = broadcasts_call(action, stream_arg, target_str, html);

    Some(Expr::new(
        Span::synthetic(),
        ExprNode::Seq {
            exprs: vec![assign, return_if, broadcast_call],
        },
    ))
}

fn sym_key(e: &Expr) -> Option<&Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value),
        _ => None,
    }
}
