//! Unknown body items recognized as Rails markers. Most Unknowns stay
//! dropped (they're emitter responsibility or future-lowerer work), but
//! a small set carry semantics that translate cleanly into method
//! definitions on the lowered class.
//!
//! Block-form lifecycle callbacks: `after_create_commit { … }` etc. They
//! surface as Unknown body items (parse_callback rejects them — no
//! symbol target, just a block). Lowered to a `def hook_name; <block-
//! body>; end`. Multiple sources can target the same hook (block-form
//! callback + broadcasts_to expansion + dependent: :destroy cascade);
//! when this lowering finds an existing method with the matching name
//! it folds the block body into that method's Seq, preserving source
//! order across sources.

use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Model, ModelBodyItem, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

use super::{fn_sig, seq, with_ty};

/// Per-model `dom_prefix` instance method returning the snake_case
/// model name as a String literal. Used by
/// `ActionView::ViewHelpers.dom_id(record)` to build CSS-id strings
/// at transpile time rather than via runtime introspection
/// (`record.class.name.downcase` previously). Skipped for abstract
/// models (`primary_abstract_class` marker present) — those are never
/// instantiated, and ApplicationRecord's lowered shape is tested
/// against the abstract-marker-only baseline.
pub(super) fn push_dom_prefix_method(methods: &mut Vec<MethodDef>, model: &Model) {
    if is_abstract_class(model) {
        return;
    }
    let prefix = crate::naming::snake_case(model.name.0.as_str());
    methods.push(MethodDef {
        name: Symbol::from("dom_prefix"),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: with_ty(
            Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Str { value: prefix } },
            ),
            Ty::Str,
        ),
        signature: Some(fn_sig(vec![], Ty::Str)),
        effects: EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
            mutates_self: false,
            block_param: None,
    });
}

/// True when the model body declares `primary_abstract_class` (Rails'
/// way of marking ApplicationRecord-shaped abstract bases). Per-model
/// synthesizers that emit instance-shaped methods skip these classes
/// since they're never instantiated.
fn is_abstract_class(model: &Model) -> bool {
    model.body.iter().any(|item| {
        if let ModelBodyItem::Unknown { expr, .. } = item {
            if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
                return args.is_empty() && method.as_str() == "primary_abstract_class";
            }
        }
        false
    })
}

/// `attr_accessor :vote` / `attr_reader :x` / `attr_writer :y` on a model
/// — virtual (non-column) attributes Rails backs with plain ivars. Lower
/// each to a getter `def name; @name; end` and/or setter `def name=(value);
/// @name = value; end`. No schema/RBS anchors the type, so they stay
/// Untyped (fine for the dynamic targets; strict targets gain a typed
/// virtual-attribute story when an app that uses them is brought up there).
/// Skips any name a column/def/association already defined, and skips
/// abstract base classes.
pub(super) fn push_attr_accessor_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    if is_abstract_class(model) {
        return;
    }
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
            continue;
        };
        let (want_reader, want_writer) = match method.as_str() {
            "attr_accessor" => (true, true),
            "attr_reader" => (true, false),
            "attr_writer" => (false, true),
            _ => continue,
        };
        for arg in args {
            let ExprNode::Lit { value: Literal::Sym { value: name } } = &*arg.node else {
                continue;
            };
            let setter = Symbol::from(format!("{}=", name.as_str()));
            if want_reader && !methods.iter().any(|m| m.name == *name) {
                methods.push(MethodDef {
                    name: name.clone(),
                    receiver: MethodReceiver::Instance,
                    params: Vec::new(),
                    body: Expr::new(expr.span, ExprNode::Ivar { name: name.clone() }),
                    signature: None,
                    effects: EffectSet::default(),
                    enclosing_class: Some(model.name.0.clone()),
                    kind: AccessorKind::AttributeReader,
                    is_async: false,
                    mutates_self: false,
                    block_param: None,
                });
            }
            if want_writer && !methods.iter().any(|m| m.name == setter) {
                let value = Symbol::from("value");
                methods.push(MethodDef {
                    name: setter,
                    receiver: MethodReceiver::Instance,
                    params: vec![Param::positional(value.clone())],
                    body: Expr::new(
                        expr.span,
                        ExprNode::Assign {
                            target: LValue::Ivar { name: name.clone() },
                            value: Expr::new(expr.span, ExprNode::Var { id: VarId(0), name: value }),
                        },
                    ),
                    signature: None,
                    effects: EffectSet::default(),
                    enclosing_class: Some(model.name.0.clone()),
                    kind: AccessorKind::AttributeWriter,
                    is_async: false,
                    mutates_self: true,
                    block_param: None,
                });
            }
        }
    }
}

/// `attribute :name, :type` declarations (the Rails Attributes API) in
/// a model body — `(name, type)` pairs, both symbol literals. The
/// 2-arg form only; `default:`-carrying declarations stay unclaimed
/// (and warned) until a fixture demands them. Shared with the view
/// lowerer's `bool_reader_names` (a `:boolean` attribute is a bool
/// reader for `f.check_box`) and the permit-writer filter (an
/// `attribute` writer is assignable).
pub(crate) fn attribute_api_decls(body: &[ModelBodyItem]) -> Vec<(Symbol, Symbol)> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "attribute" || args.len() != 2 {
            continue;
        }
        let (
            ExprNode::Lit { value: Literal::Sym { value: name } },
            ExprNode::Lit { value: Literal::Sym { value: ty } },
        ) = (&*args[0].node, &*args[1].node)
        else {
            continue;
        };
        out.push((name.clone(), ty.clone()));
    }
    out
}

/// `attribute :name, :type` — typed virtual attributes (lobsters'
/// `attribute :mod_note, :boolean` on Message, `:is_unread` on the
/// SQL-view-backed ReplyingComment). Reader is a typed ivar read;
/// the `:boolean` writer applies Rails' Type::Boolean cast over the
/// realistic value space via to_s (`"" / "0" / "false" / "f"` →
/// false, anything else → true — the form roundtrip assigns "0"/"1"
/// strings, and an uncast write would leave "0" truthy). Other types
/// assign verbatim. A custom method in the model body wins (the
/// synthesizers run before `push_user_methods`, which drops
/// collisions — same dance as attr_accessor).
pub(super) fn push_attribute_api_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    if is_abstract_class(model) {
        return;
    }
    for (name, ty_sym) in attribute_api_decls(&model.body) {
        let is_bool = ty_sym.as_str() == "boolean";
        let setter = Symbol::from(format!("{}=", name.as_str()));
        if !methods.iter().any(|m| m.name == name) {
            methods.push(MethodDef {
                name: name.clone(),
                receiver: MethodReceiver::Instance,
                params: Vec::new(),
                body: if is_bool {
                    with_ty(
                        Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() }),
                        Ty::Bool,
                    )
                } else {
                    Expr::new(Span::synthetic(), ExprNode::Ivar { name: name.clone() })
                },
                signature: if is_bool {
                    Some(super::fn_sig(vec![], Ty::Bool))
                } else {
                    None
                },
                effects: EffectSet::default(),
                enclosing_class: Some(model.name.0.clone()),
                kind: AccessorKind::AttributeReader,
                is_async: false,
                mutates_self: false,
                block_param: None,
            });
        }
        if !methods.iter().any(|m| m.name == setter) {
            let value = Symbol::from("value");
            let value_ref = Expr::new(
                Span::synthetic(),
                ExprNode::Var { id: VarId(0), name: value.clone() },
            );
            let body = if is_bool {
                // s = value.to_s
                // @name = (s == "" || s == "0" || s == "false" || s == "f" ? false : true)
                let s = Symbol::from("s");
                let s_ref = |_: ()| {
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Var { id: VarId(0), name: s.clone() },
                    )
                };
                let to_s = Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: Some(value_ref),
                        method: Symbol::from("to_s"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                );
                let assign_s = Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Var { id: VarId(0), name: s.clone() },
                        value: to_s,
                    },
                );
                let eq = |lit: &str| {
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Send {
                            recv: Some(s_ref(())),
                            method: Symbol::from("=="),
                            args: vec![Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit { value: Literal::Str { value: lit.to_string() } },
                            )],
                            block: None,
                            parenthesized: false,
                        },
                    )
                };
                let or = |left: Expr, right: Expr| {
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::BoolOp {
                            op: crate::expr::BoolOpKind::Or,
                            surface: crate::expr::BoolOpSurface::Symbol,
                            left,
                            right,
                        },
                    )
                };
                let falsey = or(or(or(eq(""), eq("0")), eq("false")), eq("f"));
                let cast = Expr::new(
                    Span::synthetic(),
                    ExprNode::If {
                        cond: falsey,
                        then_branch: with_ty(
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit { value: Literal::Bool { value: false } },
                            ),
                            Ty::Bool,
                        ),
                        else_branch: with_ty(
                            Expr::new(
                                Span::synthetic(),
                                ExprNode::Lit { value: Literal::Bool { value: true } },
                            ),
                            Ty::Bool,
                        ),
                    },
                );
                let assign = Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Ivar { name: name.clone() },
                        value: cast,
                    },
                );
                Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![assign_s, assign] })
            } else {
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Assign {
                        target: LValue::Ivar { name: name.clone() },
                        value: value_ref,
                    },
                )
            };
            methods.push(MethodDef {
                name: setter,
                receiver: MethodReceiver::Instance,
                params: vec![Param::positional(value)],
                body,
                signature: None,
                effects: EffectSet::default(),
                enclosing_class: Some(model.name.0.clone()),
                kind: AccessorKind::AttributeWriter,
                is_async: false,
                mutates_self: true,
                block_param: None,
            });
        }
    }
}

/// `primary_abstract_class` marks a model as the abstract base of a Rails
/// app. Lowered to `def self.abstract?; true; end` — the explicit form
/// spinel-blog's runtime expects.
pub(super) fn push_unknown_marker_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        if let ModelBodyItem::Unknown { expr, .. } = item {
            if let ExprNode::Send { recv: None, method, args, block: None, .. } = &*expr.node {
                if args.is_empty() && method.as_str() == "primary_abstract_class" {
                    methods.push(MethodDef {
                        name: Symbol::from("abstract?"),
                        receiver: MethodReceiver::Class,
                        params: Vec::new(),
                        body: with_ty(
                            Expr::new(
                                expr.span,
                                ExprNode::Lit { value: Literal::Bool { value: true } },
                            ),
                            Ty::Bool,
                        ),
                        signature: Some(fn_sig(vec![], Ty::Bool)),
                        effects: EffectSet::default(),
                        enclosing_class: Some(model.name.0.clone()),
                        kind: AccessorKind::AttributeReader,
                        is_async: false,
            mutates_self: false,
            block_param: None,
                    });
                }
            }
        }
    }
}

/// Look up an existing `Method` named `hook_name` and append `call` to
/// its body's Seq, OR push a new method with `call` as the body. The
/// fold preserves source order; broadcasts_to runs first so its calls
/// lead any block-form callback bodies that the next pass would add.
pub(super) fn fold_into_or_push(methods: &mut Vec<MethodDef>, model: &Model, hook_name: &str, call: Expr) {
    let hook = Symbol::from(hook_name);
    if let Some(existing) = methods.iter_mut().find(|m| m.name == hook) {
        let mut stmts = match &*existing.body.node {
            ExprNode::Seq { exprs } => exprs.clone(),
            _ => vec![existing.body.clone()],
        };
        stmts.push(call);
        existing.body = seq(stmts);
    } else {
        methods.push(MethodDef {
            name: hook,
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: call,
            signature: Some(fn_sig(vec![], Ty::Nil)),
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}

/// Lifecycle hook names that appear as block-form Unknown items. Names
/// not in this set fall through to plain Unknown (they're future
/// lowerer or emit work). Includes the `_commit` variants Rails sugar
/// adds beyond the raw `after_commit` hook in `CallbackHook`.
pub(super) const BLOCK_CALLBACK_HOOKS: &[&str] = &[
    "before_validation",
    "after_validation",
    "before_save",
    "after_save",
    "before_create",
    "after_create",
    "before_update",
    "after_update",
    "before_destroy",
    "after_destroy",
    "after_commit",
    "after_rollback",
    "after_create_commit",
    "after_update_commit",
    "after_destroy_commit",
    "after_save_commit",
];

pub(super) fn push_block_callback_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node else {
            continue;
        };
        if !args.is_empty() {
            continue;
        }
        let hook = method.as_str();
        if !BLOCK_CALLBACK_HOOKS.contains(&hook) {
            continue;
        }
        let ExprNode::Lambda { body: lambda_body, .. } = &*block.node else {
            continue;
        };

        // Translate Rails-API broadcast calls (`assoc.broadcast_replace_to(...)`
        // etc.) inside the block body to spinel-shape `Broadcasts.<action>(...)`
        // calls. Other content passes through unchanged.
        let mut lambda_body = super::broadcasts::rewrite_rails_broadcast_calls(
            lambda_body.clone(),
            model,
        );
        // The rewrite synthesizes wrapper nodes; whatever it left
        // span-less attributes to the hook declaration. Source subtrees
        // spliced through keep their exact spans.
        lambda_body.inherit_span(expr.span);

        let hook_sym = method.clone();
        if let Some(existing) = methods.iter_mut().find(|m| m.name == hook_sym) {
            // Fold this block's body into the existing method, preserving
            // source order (existing body's stmts first, then this block's).
            let mut stmts = match &*existing.body.node {
                ExprNode::Seq { exprs } => exprs.clone(),
                _ => vec![existing.body.clone()],
            };
            match &*lambda_body.node {
                ExprNode::Seq { exprs } => stmts.extend(exprs.clone()),
                _ => stmts.push(lambda_body.clone()),
            }
            existing.body = seq(stmts);
        } else {
            methods.push(MethodDef {
                name: hook_sym,
                receiver: MethodReceiver::Instance,
                params: Vec::new(),
                body: lambda_body,
                signature: None,
                effects: EffectSet::default(),
                enclosing_class: Some(model.name.0.clone()),
                kind: AccessorKind::Method,
                is_async: false,
            mutates_self: false,
            block_param: None,
            });
        }
    }
}
