//! Unknown body items recognized as Rails markers. Most Unknowns stay
//! dropped (they're emitter responsibility or future-lowerer work), but
//! a small set carry semantics that translate cleanly into method
//! definitions on the lowered class.
//!
//! Lifecycle callbacks, both forms: symbol-form declarations
//! (`before_save :check_session_token` — ingested as
//! `ModelBodyItem::Callback`) lower to self-calls inside a `def
//! hook_name` override of the runtime Base's no-op hook, and
//! block-form ones (`after_create_commit { … }` — Unknown body items,
//! parse_callback rejects them) lower to `def hook_name; <block-
//! body>; end`. Multiple sources can target the same hook (either
//! callback form + broadcasts_to expansion + dependent: :destroy
//! cascade); when this lowering finds an existing method with the
//! matching name it folds the new body into that method's Seq,
//! preserving source order across sources.

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

/// `self.<col> ||= value` (string column) → `if self.<col>.blank?
/// then self.<col> = value end` — see the call-site note in
/// `push_block_callback`. Recursive over the whole body; only fires
/// for `LValue::Attr` targets on `self` naming a schema string
/// column.
fn rewrite_column_or_assign(e: &mut Expr, model: &Model) {
    use crate::expr::{LValue, OpAssignOp};
    let hit = match &*e.node {
        ExprNode::OpAssign {
            target: LValue::Attr { recv, name },
            op: OpAssignOp::OrOr,
            value,
        } if matches!(&*recv.node, ExprNode::SelfRef)
            && matches!(model.attributes.fields.get(name), Some(crate::ty::Ty::Str)) =>
        {
            Some((name.clone(), value.clone()))
        }
        _ => None,
    };
    if let Some((name, value)) = hit {
        let span = e.span;
        let self_read = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::SelfRef)),
                method: name.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let blank = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(self_read),
                method: Symbol::from("blank?"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let assign = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::SelfRef)),
                method: Symbol::from(format!("{}=", name.as_str())),
                args: vec![value],
                block: None,
                parenthesized: false,
            },
        );
        *e.node = ExprNode::If {
            cond: blank,
            then_branch: assign,
            else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        };
        e.ty = None;
        return;
    }
    e.node.for_each_child_mut(&mut |c| rewrite_column_or_assign(c, model));
}

/// Look up an existing `Method` named `hook_name` and append `call` to
/// its body's Seq, OR push a new method with `call` as the body. The
/// fold preserves source order; broadcasts_to runs first so its calls
/// lead any block-form callback bodies that the next pass would add.
pub(crate) fn fold_into_or_push(methods: &mut Vec<MethodDef>, model: &Model, hook_name: &str, call: Expr) {
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


/// `belongs_to :creator, class_name: "User", default: -> { Current.user }`
/// → a `before_validation` statement:
///
///   self.creator = Current.user if @creator_id.nil? || @creator_id == 0
///
/// Rails registers this as a `before_validation` at declaration time
/// (`Builder::BelongsTo.add_default_callbacks`), whose body is
/// `association(name).default(&block)` — "write the block's value when
/// the reader is nil". Without it every `Message.create!` that did not
/// name a creator failed the presence validation the SAME declaration
/// installs, which reads as the app being wrong about its own model:
/// campfire creates messages, rooms and boosts from `Current.user`
/// alone in seven tests.
///
/// The guard is on the FOREIGN KEY, not the reader. `creator.nil?`
/// would issue a SELECT on every validation (the reader is the
/// row-loading one), and the two agree by construction — the synthesized
/// writer stores 0 for nil, which is the same pair `inline_belongs_to_
/// check` tests in validations.rs.
///
/// The ASSIGNMENT goes through the writer rather than the ivar, because
/// the default lambda yields a RECORD (`Current.user`) and `creator=`
/// is what turns one into an id — including the nil case, which it
/// stores as 0 and the validation then rejects, exactly as Rails
/// rejects a default that evaluated to nil.
fn push_belongs_to_defaults(methods: &mut Vec<MethodDef>, model: &Model) {
    for assoc in model.associations() {
        let crate::dialect::Association::BelongsTo {
            name, foreign_key, default: Some(default), ..
        } = assoc
        else {
            continue;
        };
        let span = default.span;
        let fk = Expr::new(span, ExprNode::Ivar { name: foreign_key.clone() });
        let unset = Expr::new(
            span,
            ExprNode::BoolOp {
                op: crate::expr::BoolOpKind::Or,
                surface: crate::expr::BoolOpSurface::default(),
                left: Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(fk.clone()),
                        method: Symbol::from("nil?"),
                        args: vec![],
                        block: None,
                        parenthesized: false,
                    },
                ),
                right: Expr::new(
                    span,
                    ExprNode::Send {
                        recv: Some(fk),
                        method: Symbol::from("=="),
                        args: vec![Expr::new(span, ExprNode::Lit { value: Literal::Int { value: 0 } })],
                        block: None,
                        parenthesized: false,
                    },
                ),
            },
        );
        let assign = Expr::new(
            span,
            ExprNode::Send {
                recv: Some(Expr::new(span, ExprNode::SelfRef)),
                method: Symbol::from(format!("{}=", name.as_str())),
                args: vec![default.clone()],
                block: None,
                parenthesized: false,
            },
        );
        let stmt = Expr::new(
            span,
            ExprNode::If {
                cond: unset,
                then_branch: assign,
                else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            },
        );
        fold_into_or_push(methods, model, "before_validation", stmt);
    }
}

/// Lifecycle hook names that appear as block-form Unknown items. Names
/// not in this set fall through to plain Unknown (they're future
/// lowerer or emit work). Includes the `_commit` variants Rails sugar
/// adds beyond the raw `after_commit` hook in `CallbackHook`, plus
/// `after_initialize` (fires on construction AND hydration — the
/// runtime hook call is appended by `synth_initialize` / the
/// hydration factories when a model declares it). `pub(crate)`: the
/// concern-items ingest keeps block-form callbacks by this list.
pub(crate) const BLOCK_CALLBACK_HOOKS: &[&str] = &[
    "after_initialize",
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

/// Lower lifecycle callbacks — both the symbol-form declarations
/// ingest recognized (`ModelBodyItem::Callback`, e.g. `before_save
/// :check_session_token`) and the block-form ones that surface as
/// Unknown items — into `def <hook>` overrides of the runtime Base's
/// no-op hooks. One body walk so declaration order is preserved
/// across both forms when they target the same hook.
pub(super) fn push_callback_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    // AHEAD of the declared callbacks: Rails registers the
    // `belongs_to … default:` callback when the association is
    // declared, and campfire declares its associations above its
    // callbacks. A user callback that reads `creator` therefore sees
    // the default already applied, which is the order that matters.
    push_belongs_to_defaults(methods, model);
    for item in &model.body {
        match item {
            ModelBodyItem::Callback { callback, .. } => {
                push_symbol_callback(methods, model, callback, item.span());
            }
            ModelBodyItem::Unknown { expr, .. } => {
                push_block_callback(methods, model, expr);
            }
            _ => {}
        }
    }
}

/// Symbol-form callback → self-calls folded into the hook override.
/// `on:` restrictions lower structurally: `after_commit ..., on:
/// :create` targets the runtime's `after_create_commit` hook, and
/// validation hooks get a `new_record?` guard (accurate at validation
/// time — the insert hasn't happened yet). Ingest already rejected
/// every (hook, on) pair this match doesn't cover, plus `if:`/
/// `unless:` conditions.
fn push_symbol_callback(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    cb: &crate::dialect::Callback,
    span: Span,
) {
    use crate::dialect::{CallbackHook as Hook, CallbackOn as On};

    if cb.condition.is_some() {
        return;
    }
    let hook_name = match (cb.hook, cb.on) {
        (Hook::AfterCommit, Some(On::Create)) => "after_create_commit",
        (Hook::AfterCommit, Some(On::Update)) => "after_update_commit",
        (Hook::AfterCommit, Some(On::Destroy)) => "after_destroy_commit",
        (hook, _) => hook_method_name(hook),
    };
    let self_call = |name: &Symbol| {
        Expr::new(
            span,
            ExprNode::Send {
                recv: None,
                method: name.clone(),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        )
    };

    if matches!(cb.hook, Hook::BeforeValidation | Hook::AfterValidation) && cb.on.is_some() {
        // Validations never run on destroy; ingest rejects this.
        let Some(on) = cb.on else { return };
        let body = seq(cb.targets.iter().map(self_call).collect());
        let Some(body) = guard_validation_on(body, on, span) else { return };
        fold_into_or_push(methods, model, hook_name, body);
    } else {
        for target in &cb.targets {
            fold_into_or_push(methods, model, hook_name, self_call(target));
        }
    }
}

/// Wrap a validation-hook body in the `new_record?` guard its `on:`
/// restriction lowers to — accurate at validation time, since the
/// insert hasn't happened yet. `on: :destroy` has no reading here
/// (validations never run on destroy) and returns None: drop the
/// callback rather than run it in the wrong circumstances.
fn guard_validation_on(
    body: Expr,
    on: crate::dialect::CallbackOn,
    span: Span,
) -> Option<Expr> {
    use crate::dialect::CallbackOn as On;

    let new_record = Expr::new(
        span,
        ExprNode::Send {
            recv: None,
            method: Symbol::from("new_record?"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let cond = match on {
        On::Create => new_record,
        On::Update => Expr::new(
            span,
            ExprNode::Send {
                recv: Some(new_record),
                method: Symbol::from("!"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        ),
        On::Destroy => return None,
    };
    let mut guarded = Expr::new(
        span,
        ExprNode::If {
            cond,
            then_branch: body,
            else_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
        },
    );
    guarded.inherit_span(span);
    Some(guarded)
}

/// The runtime Base hook method a `CallbackHook` overrides when no
/// `on:` remap applies. Names match the no-op definitions in
/// `runtime/ruby/active_record/base.rb`.
fn hook_method_name(hook: crate::dialect::CallbackHook) -> &'static str {
    use crate::dialect::CallbackHook as Hook;
    match hook {
        Hook::BeforeValidation => "before_validation",
        Hook::AfterValidation => "after_validation",
        Hook::BeforeSave => "before_save",
        Hook::AfterSave => "after_save",
        Hook::BeforeCreate => "before_create",
        Hook::AfterCreate => "after_create",
        Hook::BeforeUpdate => "before_update",
        Hook::AfterUpdate => "after_update",
        Hook::BeforeDestroy => "before_destroy",
        Hook::AfterDestroy => "after_destroy",
        Hook::AfterCommit => "after_commit",
        Hook::AfterRollback => "after_rollback",
    }
}

/// The `on:` restriction from a block-form callback's option hash.
/// None means "a shape this lowering won't run": not an `on:` hash at
/// all, an unmodeled value, or extra options (`if:`/`unless:`) whose
/// conditions would be silently dropped.
fn block_callback_on(arg: &Expr) -> Option<crate::dialect::CallbackOn> {
    use crate::dialect::CallbackOn as On;

    let ExprNode::Hash { entries, .. } = &*arg.node else { return None };
    let [(k, v)] = &entries[..] else { return None };
    let ExprNode::Lit { value: Literal::Sym { value: key } } = &*k.node else { return None };
    let ExprNode::Lit { value: Literal::Sym { value: val } } = &*v.node else { return None };
    if key.as_str() != "on" {
        return None;
    }
    match val.as_str() {
        "create" => Some(On::Create),
        "update" => Some(On::Update),
        "destroy" => Some(On::Destroy),
        // `on: [:create, :update]` array form: not modeled.
        _ => None,
    }
}

fn push_block_callback(methods: &mut Vec<MethodDef>, model: &Model, expr: &Expr) {
    {
        let ExprNode::Send { recv: None, method, args, block: Some(block), .. } = &*expr.node else {
            return;
        };
        // `before_validation on: :create do … end` — the block form
        // carries its restriction as an option hash where the symbol
        // form carries it as a keyword. Anything else in that hash
        // (`if:`/`unless:`) drops the callback, matching ingest's
        // rejection for the symbol form.
        let on = match &args[..] {
            [] => None,
            [opts] => match block_callback_on(opts) {
                Some(on) => Some(on),
                None => return,
            },
            _ => return,
        };
        let hook = method.as_str();
        if !BLOCK_CALLBACK_HOOKS.contains(&hook) {
            return;
        }
        // Same structural lowering as the symbol form: after_commit
        // retargets the per-lifecycle hook, validation hooks keep
        // their name and gain a `new_record?` guard below. Rails
        // doesn't accept `on:` on the remaining hooks.
        let hook_name = match (hook, on) {
            (_, None) => hook,
            ("after_commit", Some(crate::dialect::CallbackOn::Create)) => "after_create_commit",
            ("after_commit", Some(crate::dialect::CallbackOn::Update)) => "after_update_commit",
            ("after_commit", Some(crate::dialect::CallbackOn::Destroy)) => "after_destroy_commit",
            ("before_validation" | "after_validation", Some(_)) => hook,
            _ => return,
        };
        let ExprNode::Lambda { body: lambda_body, .. } = &*block.node else {
            return;
        };

        // Translate Rails-API broadcast calls (`assoc.broadcast_replace_to(...)`
        // etc.) inside the block body to spinel-shape `Broadcasts.<action>(...)`
        // calls. Other content passes through unchanged.
        let mut lambda_body = super::broadcasts::rewrite_rails_broadcast_calls(
            lambda_body.clone(),
            model,
        );
        // `self.<col> ||= v` on a string column → blank-guarded assign.
        // Rails' new-record attributes are nil, so `||=` means "set
        // unless set"; this runtime's storage defaults string slots to
        // "" (strict targets assign every field), which is truthy and
        // would starve the idiom (lobsters' Token concern generates
        // its unique token exactly this way in after_initialize).
        // Blank-on-"" IS the faithful reading of "unset" under the
        // ""-default storage model.
        rewrite_column_or_assign(&mut lambda_body, model);
        // The rewrite synthesizes wrapper nodes; whatever it left
        // span-less attributes to the hook declaration. Source subtrees
        // spliced through keep their exact spans.
        lambda_body.inherit_span(expr.span);

        if matches!(hook, "before_validation" | "after_validation") {
            if let Some(on) = on {
                let Some(guarded) = guard_validation_on(lambda_body, on, expr.span) else {
                    return;
                };
                lambda_body = guarded;
            }
        }

        let hook_sym = Symbol::from(hook_name);
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
