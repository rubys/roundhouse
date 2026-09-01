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

        let mut create_call = broadcasts_call(
            create_action,
            stream_expr.clone(),
            create_target,
            Some(html_partial.clone()),
        );
        let mut update_call = broadcasts_call(
            BroadcastAct::Replace,
            stream_expr.clone(),
            canonical_target.clone(),
            Some(html_partial),
        );
        let mut destroy_call = broadcasts_call(
            BroadcastAct::Remove,
            stream_expr,
            canonical_target,
            None,
        );

        // All three lifecycle expansions attribute to the one
        // `broadcasts_to` declaration; the channel lambda's source
        // subtrees keep their exact spans.
        create_call.inherit_span(expr.span);
        update_call.inherit_span(expr.span);
        destroy_call.inherit_span(expr.span);

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
        entries.push((lit_sym(Symbol::from("html")), wrap_broadcast_render(h)));
    }
    let kwargs = Expr::new(Span::synthetic(), ExprNode::Hash { entries, kwargs: true });
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

/// `"#{dom_prefix()}_#{dom_record_key()}"` — the canonical per-record
/// DOM target Rails turbo uses on update + destroy regardless of
/// `target:` option. Through the SYNTHESIZED identity methods rather
/// than a static singular + `@id`, because that static spelling was a
/// second copy of dom_id's semantics and it drifted twice at once: an
/// STI row's prefix is the subclass's (`rooms_open`, dispatched on the
/// type column), and a model with its own `to_key` keys rows by it
/// (campfire's Message → client_message_id — a `broadcast_remove`
/// aimed at `message_#{@id}` would MISS every row the pages render).
fn canonical_record_target(_class_name: &ClassId) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp {
            parts: vec![
                crate::expr::InterpPart::Expr { expr: dom_identity_call(None, "dom_prefix") },
                crate::expr::InterpPart::Text { value: "_".to_string() },
                crate::expr::InterpPart::Expr {
                    expr: dom_identity_call(None, "dom_record_key"),
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
    // The render answers String by contract, but the model lowering's
    // typing registry has no Views surface (views lower AFTER models),
    // so a bare stamp gets overwritten to `Var` by the body-typer and
    // rust's owned-String → `&str` borrow coercion goes blind. `Cast`
    // is the IR's assertion node for exactly this: the typer answers
    // its target type, and the emitters that coerce args peek through
    // it (ruby renders it as identity here — the inner ty is not
    // poly).
    let mut html = Expr::new(
        Span::synthetic(),
        ExprNode::Cast { value: html, target_ty: crate::ty::Ty::Str },
    );
    html.ty = Some(crate::ty::Ty::Str);
    let vh_const = || {
        Expr::new(
            Span::synthetic(),
            ExprNode::Const {
                path: vec![
                    Symbol::from("ActionView"),
                    Symbol::from("ViewHelpers"),
                ],
            },
        )
    };
    let mut armed = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(vh_const()),
            method: Symbol::from("begin_broadcast_render"),
            args: vec![],
            block: None,
            parenthesized: true,
        },
    );
    armed.ty = Some(crate::ty::Ty::Str);
    let mut call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(vh_const()),
            method: Symbol::from("broadcast_render"),
            args: vec![armed, html],
            block: None,
            parenthesized: true,
        },
    );
    call.ty = Some(crate::ty::Ty::Str);
    call
}

/// A zero-arg call to one of the synthesized dom-identity methods.
/// Parenthesized, for the same TS-collapse reason
/// `ViewHelpers.dom_id` spells `record.dom_prefix()` with parens.
fn dom_identity_call(recv: Option<Expr>, name: &str) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv,
            method: Symbol::from(name),
            args: vec![],
            block: None,
            parenthesized: true,
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
/// ```text
/// parent = <assoc>
/// return if parent.nil?
/// Broadcasts.<action>(stream: <stream>,
///                     target: "<sing>_#{parent.id}",
///                     html: Views::<Plur>.<sing>(parent))
/// ```
///
/// `<assoc>` must name a `belongs_to` association on `model` so the
/// target class — and from it the stream's per-record DOM target +
/// partial render — is resolvable. `<call> rescue nil` modifiers
/// strip away: the explicit `parent.nil?` early-return covers the
/// "association is missing" case the rescue was guarding against.
///
/// Other shapes (non-belongs_to receiver, unknown method) pass
/// through unchanged so the emitter still produces parseable Ruby.
pub(crate) fn rewrite_rails_broadcast_calls(expr: Expr, model: &Model) -> Expr {
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
    let ExprNode::Send { recv, method, args, block: None, .. } = &*expr.node else {
        return None;
    };
    let action = broadcast_action(method.as_str())?;
    // `broadcast_append_to room, :messages, target: […]` — the
    // IMPERATIVE form, called on SELF from an ordinary method body
    // (campfire writes its broadcasts that way, in a concern). The
    // association form below is the one a `after_*_commit` block uses.
    let Some(recv) = recv else {
        return rewrite_self_broadcast(action, args, model, expr.span);
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
    let _ = parent_id;
    // Through the synthesized identity methods, not `{singular}_` +
    // `.id` — the same one-spelling rule canonical_record_target
    // documents (STI prefixes, `to_key`-keyed rows).
    let target_str = Expr::new(
        Span::synthetic(),
        ExprNode::StringInterp {
            parts: vec![
                crate::expr::InterpPart::Expr {
                    expr: dom_identity_call(Some(var_ref(parent_sym.clone())), "dom_prefix"),
                },
                crate::expr::InterpPart::Text { value: "_".to_string() },
                crate::expr::InterpPart::Expr {
                    expr: dom_identity_call(Some(var_ref(parent_sym.clone())), "dom_record_key"),
                },
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

fn broadcast_action(method: &str) -> Option<BroadcastAct> {
    match method {
        "broadcast_replace_to" => Some(BroadcastAct::Replace),
        "broadcast_append_to" => Some(BroadcastAct::Append),
        "broadcast_prepend_to" => Some(BroadcastAct::Prepend),
        "broadcast_remove_to" => Some(BroadcastAct::Remove),
        _ => None,
    }
}

/// The local the record streamable binds to. Evaluated ONCE: the stream
/// name and the DOM target both read its id, and an association reader
/// is a query on most targets.
fn owner_local() -> Symbol {
    Symbol::from("bc_owner")
}

/// `broadcast_<action>_to <streamables…>, target: <t>` with an implicit
/// self receiver → the same `Broadcasts.<action>(stream:, target:,
/// html:)` shape the declarative macro expands to.
///
/// Semantics measured against turbo-rails 2.0.23 + Rails 8.1:
///   * the stream is `Turbo::StreamsChannel.stream_name_from` over the
///     streamables — see `lower::broadcasts::stream_name` for how we
///     spell it without GlobalIDs;
///   * `target:` runs through `convert_to_turbo_stream_dom_id`, so an
///     array is `dom_id(*array)` — `[room, :messages]` is
///     `"messages_room_1"`, prefix FIRST — and a string passes through;
///   * with no `target:`, append/prepend/replace default to
///     `model_name.plural` and remove defaults to `dom_id(self)`;
///   * with no `partial:`, the payload is the record's own partial with
///     itself as the local, which is exactly `views_render_self`.
fn rewrite_self_broadcast(
    action: BroadcastAct,
    args: &[Expr],
    model: &Model,
    span: Span,
) -> Option<Expr> {
    let (positional, opts) = split_trailing_kwargs(args);
    if positional.is_empty() {
        return None;
    }
    // Rendering options that would change WHAT is rendered are not
    // modeled yet; taking the default partial anyway would broadcast
    // the wrong markup, which is worse than not broadcasting.
    for (key, _) in &opts {
        match key.as_str() {
            "target" => {}
            other => {
                return decline(
                    span,
                    &format!("broadcast_{}_to option `{other}:`", action.method_name()),
                )
            }
        }
    }

    let (parts, owner) = streamables(positional, model, span)?;
    let stream = crate::lower::broadcasts::stream_name(&parts);

    let target = match opts.iter().find(|(k, _)| k.as_str() == "target") {
        Some((_, value)) => dom_target(value, model, owner.as_ref(), span)?,
        None => match action {
            BroadcastAct::Remove => canonical_record_target(&model.name),
            _ => lit_str(crate::naming::pluralize_snake(model.name.0.as_str())),
        },
    };

    let html = match action {
        BroadcastAct::Remove => None,
        _ => Some(views_render_self(&model.name)),
    };
    let call = broadcasts_call(action, stream, target, html);

    // A record streamable is read through an association, so bind it
    // once and skip the broadcast when it is missing — same guard the
    // association form uses, and for the same reason.
    let Some(owner) = owner else {
        return Some(call);
    };
    Some(Expr::new(
        span,
        ExprNode::Seq {
            exprs: vec![
                Expr::new(
                    span,
                    ExprNode::Assign {
                        target: LValue::Var { id: VarId(0), name: owner_local() },
                        value: owner.expr,
                    },
                ),
                return_if_nil(owner_local()),
                call,
            ],
        },
    ))
}

/// A resolved record streamable: which class it names, and the
/// expression that produced it.
struct OwnerRef {
    singular: String,
    expr: Expr,
}

/// Classify each streamable argument. A literal is its own text; a
/// bare name that is one of this model's `belongs_to` associations is
/// that record. Anything else declines the whole call — a stream name
/// we guessed at is a stream nobody is subscribed to.
fn streamables(
    args: &[Expr],
    model: &Model,
    span: Span,
) -> Option<(Vec<crate::lower::broadcasts::Streamable>, Option<OwnerRef>)> {
    use crate::lower::broadcasts::Streamable;
    let mut parts = Vec::new();
    let mut owner: Option<OwnerRef> = None;
    for arg in args {
        match &*arg.node {
            ExprNode::Lit { value: Literal::Sym { value } } => {
                parts.push(Streamable::Literal(value.as_str().to_string()))
            }
            ExprNode::Lit { value: Literal::Str { value } } => {
                parts.push(Streamable::Literal(value.clone()))
            }
            // `broadcast_append_to self` — Rails' own shorthand for a
            // per-record stream. No association read, so no nil guard.
            ExprNode::SelfRef => parts.push(Streamable::Record {
                singular: crate::naming::snake_case(model.name.0.as_str()),
                id: Expr::new(Span::synthetic(), ExprNode::Ivar { name: Symbol::from("id") }),
            }),
            ExprNode::Send { recv: None, method: name, args: a, block: None, .. }
                if a.is_empty() =>
            {
                let target = model.associations().find_map(|assoc| match assoc {
                    Association::BelongsTo { name: assoc_name, target, .. }
                        if assoc_name == name =>
                    {
                        Some(target.clone())
                    }
                    _ => None,
                });
                let Some(target) = target else {
                    return decline_opt(span, &format!("streamable `{name}` is not a belongs_to"));
                };
                if owner.is_some() {
                    return decline_opt(span, "more than one record streamable");
                }
                let singular = crate::naming::snake_case(target.0.as_str());
                parts.push(Streamable::Record {
                    singular: singular.clone(),
                    id: read_id(var_ref(owner_local())),
                });
                owner = Some(OwnerRef { singular, expr: arg.clone() });
            }
            _ => return decline_opt(span, "streamable is not a literal or a belongs_to"),
        }
    }
    Some((parts, owner))
}

/// `target:` → the DOM id turbo would compute. An array is
/// `dom_id(record, prefix)` — prefix FIRST, measured — and a literal
/// string is itself.
fn dom_target(value: &Expr, _model: &Model, owner: Option<&OwnerRef>, span: Span) -> Option<Expr> {
    match &*value.node {
        ExprNode::Lit { value: Literal::Str { .. } } => Some(value.clone()),
        ExprNode::Array { elements, .. } => {
            let [record, prefix] = elements.as_slice() else {
                return decline(span, "target: array is not [record, prefix]");
            };
            let Some(prefix) = literal_text(prefix) else {
                return decline(span, "target: prefix is not a literal");
            };
            // The record must be the one already bound; a second
            // association read here would be a second query AND could
            // disagree with the stream it is paired with.
            let _owner = owner.filter(|o| same_bare_name(record, &o.expr))?;
            // `"#{prefix}_#{owner.dom_prefix()}_#{owner.dom_record_key()}"`
            // — dom_id(owner, prefix) through the synthesized identity
            // methods, so an STI owner names the subclass the page
            // names (see canonical_record_target's note).
            Some(Expr::new(
                span,
                ExprNode::StringInterp {
                    parts: vec![
                        crate::expr::InterpPart::Text { value: format!("{prefix}_") },
                        crate::expr::InterpPart::Expr {
                            expr: dom_identity_call(Some(var_ref(owner_local())), "dom_prefix"),
                        },
                        crate::expr::InterpPart::Text { value: "_".to_string() },
                        crate::expr::InterpPart::Expr {
                            expr: dom_identity_call(
                                Some(var_ref(owner_local())),
                                "dom_record_key",
                            ),
                        },
                    ],
                },
            ))
        }
        _ => decline(span, "target: is not a string or [record, prefix]"),
    }
}

fn same_bare_name(a: &Expr, b: &Expr) -> bool {
    let name_of = |e: &Expr| match &*e.node {
        ExprNode::Send { recv: None, method, args, block: None, .. } if args.is_empty() => {
            Some(method.clone())
        }
        _ => None,
    };
    match (name_of(a), name_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn literal_text(e: &Expr) -> Option<String> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value.as_str().to_string()),
        ExprNode::Lit { value: Literal::Str { value } } => Some(value.clone()),
        _ => None,
    }
}

fn read_id(recv: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(recv),
            method: Symbol::from("id"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    )
}

fn return_if_nil(name: Symbol) -> Expr {
    let cond = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(var_ref(name)),
            method: Symbol::from("nil?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond,
            then_branch: Expr::new(Span::synthetic(), ExprNode::Return { value: nil_lit() }),
            else_branch: nil_lit(),
        },
    )
}

fn lit_str(value: String) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Str { value } },
    )
}

/// Leave the call alone and file the reason. The emitted code then
/// still parses and still fails loudly at run time, with a ledger line
/// naming what we did not model — better than a broadcast to a stream
/// name we invented.
fn decline<T>(span: Span, what: &str) -> Option<T> {
    crate::emit::diagnostics::push(crate::lower::residue_diagnostic(
        "broadcast",
        what,
        span,
        "broadcast call not lowered",
        format!("`{what}` is not modeled — the call is emitted as written"),
    ));
    None
}

fn decline_opt<T>(span: Span, what: &str) -> Option<T> {
    decline(span, what)
}

fn split_trailing_kwargs(args: &[Expr]) -> (&[Expr], Vec<(Symbol, Expr)>) {
    let Some(last) = args.last() else {
        return (args, Vec::new());
    };
    let ExprNode::Hash { entries, .. } = &*last.node else {
        return (args, Vec::new());
    };
    let opts = entries
        .iter()
        .filter_map(|(k, v)| sym_key(k).map(|s| (s.clone(), v.clone())))
        .collect();
    (&args[..args.len() - 1], opts)
}

fn sym_key(e: &Expr) -> Option<&Symbol> {
    match &*e.node {
        ExprNode::Lit { value: Literal::Sym { value } } => Some(value),
        _ => None,
    }
}
