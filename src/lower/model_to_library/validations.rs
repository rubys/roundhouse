//! Validations: lower `validates :attr, presence: true, length: { ... }` into
//! a single `def validate` body. Each rule expands to inline IR (`if cond
//! then errors << "msg" end`) rather than a helper-call into the runtime
//! Validations module — the Phase 2.5(a) lowerer per docs/archive/rust-migration-plan.md.
//!
//! Inline expansion wins three ways: (1) error messages are string-literal
//! constants, no runtime interpolation, (2) typed targets (Rust, Crystal,
//! strict TS) avoid the `untyped value` channel that the Validations module
//! forces, (3) every target gets the same expansion — no per-target adapter
//! for the Validations module's dispatch shape.
//!
//! One top-level `def validate` per model; rules across multiple attrs each
//! append their own stmt block to the body.

use crate::dialect::{AccessorKind, Association, MethodDef, MethodReceiver, Model, ValidationRule};
use crate::effect::EffectSet;
use crate::expr::{ArrayStyle, BoolOpKind, BoolOpSurface, Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::span::Span;
use crate::ty::Ty;

use super::{fn_sig, seq, ErrorsReaders};

pub(super) fn push_validate_method(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    errors_readers: ErrorsReaders,
    runtime_relation: super::RuntimeRelation,
    primary_key: &Symbol,
) {
    let model_msgs = Messages(Some(model), None);
    let mut stmts: Vec<Expr> = Vec::new();

    // One pass in declaration order: Rails checks a `belongs_to` where it is declared, among the `validates`, so its error lists there.
    for item in &model.body {
        let span = item.span();
        match item {
            crate::dialect::ModelBodyItem::Validation { validation: v, .. } => {
                // The validation's own `message:` replaces every error's default.
                let msgs = Messages(Some(model), v.message.as_deref());
                // Column type from the model's attributes row (when present).
                // Lets the per-rule generator skip dead `is_a?(Array)` branches
                // for fields the schema declares as `Str` — tsc narrows
                // `Array.isArray(stringField)` to `never` and rejects the
                // subsequent `.length` access.
                let attr_ty = model.attributes.fields.get(&v.attribute);
                // `validates :user, presence: true` on a `belongs_to :user` (or
                // `has_one`) names the ASSOCIATION, and Rails asks its reader —
                // `user.blank?`, nil for a record. There is no `@user` slot: the
                // writer sets `@user_id` and caches the record, so the column
                // check below read an ivar nothing writes and rejected every
                // record. The 2023 lobsters snapshot's ReadRibbon validates this
                // way, and its story page (reached once `around_action` ran)
                // could never save a ribbon.
                let is_assoc = model.associations().any(|a| {
                    matches!(a, Association::BelongsTo { name, .. } | Association::HasOne { name, .. }
                        if name == &v.attribute)
                });
                // `validates :image, presence: true` on a `has_one_attached
                // :image` (or `has_many_attached :images`) names the
                // ATTACHMENT. There is no `@image` slot either — the reader
                // builds an `ActiveStorage::Attached` proxy — so the column
                // check rejected every record, attached or not. Rails asks
                // the proxy `blank?`: nothing attached, staged or saved.
                let attachment = attachment_kind(model, &v.attribute);
                for rule in &v.rules {
                    let mut produced = if is_assoc && matches!(rule, ValidationRule::Presence) {
                        let reader = Expr::new(
                            Span::synthetic(),
                            ExprNode::Send {
                                recv: None,
                                method: v.attribute.clone(),
                                args: vec![],
                                block: None,
                                parenthesized: false,
                            },
                        );
                        vec![if_with_nil_else(
                            send(reader, "nil?", vec![]),
                            errors_push(v.attribute.as_str(), msgs.error(v.attribute.as_str(), "blank", &Default::default())),
                        )]
                    } else if let (Some(kind), ValidationRule::Presence) = (attachment, rule) {
                        vec![if_with_nil_else(
                            attachment_blank_cond(model, &v.attribute, kind),
                            errors_push(v.attribute.as_str(), msgs.error(v.attribute.as_str(), "blank", &Default::default())),
                        )]
                    } else if matches!(rule, ValidationRule::Acceptance { .. })
                        && super::markers::synthesized_boolean_attribute(model, &v.attribute)
                    {
                        match runtime_relation {
                            // The ruby family's writer stores Rails' cast
                            // (`true`, `false` or nil), which the check
                            // compares as written.
                            super::RuntimeRelation::Present => {
                                validation_rule_to_calls(msgs, &v.attribute, rule, None, runtime_relation)
                            }
                            // A strict target's field turns nil and `""`
                            // into `false`, which the check would reject
                            // where Rails skips it. Reported per target by
                            // `lower::validations::acceptance_gaps_for_target`.
                            super::RuntimeRelation::Absent => Vec::new(),
                        }
                    } else if let ValidationRule::Uniqueness { scope, case_sensitive, allow_nil, allow_blank } = rule {
                        match runtime_relation {
                            super::RuntimeRelation::Present => vec![inline_uniqueness_check(
                                msgs, model, primary_key, &v.attribute, scope, *case_sensitive, *allow_nil, *allow_blank,
                            )],
                            // Reported per target by
                            // `relation_residue::uniqueness_gaps_for_target`.
                            super::RuntimeRelation::Absent => Vec::new(),
                        }
                    } else {
                        validation_rule_to_calls(msgs, &v.attribute, rule, attr_ty, runtime_relation)
                    };
                    if v.if_cond.is_some() || v.unless_cond.is_some() {
                        let body = if produced.len() == 1 {
                            produced.pop().unwrap()
                        } else {
                            seq(produced)
                        };
                        produced = vec![with_condition_guard(body, &v.if_cond, &v.unless_cond)];
                    }
                    for mut check in produced {
                        // Each expanded check attributes to its `validates` line.
                        check.inherit_span(span);
                        stmts.push(check);
                    }
                }
            }
            crate::dialect::ModelBodyItem::Association { assoc, .. } => {
                // Rails 5+ default: every `belongs_to` requires the associated
                // record to exist before save, checked through the association
                // reader (`inline_belongs_to_check`).
                if let Association::BelongsTo {
                    name, foreign_key, optional: false, polymorphic, ..
                } = assoc
                {
                    let mut check = inline_belongs_to_check(model_msgs, name, foreign_key, *polymorphic);
                    // `accepts_nested_attributes_for` (`ingest::nested_attributes`)
                    // builds the record before the save, which writes its id:
                    // Rails checks the object it built, so a pending one passes,
                    // and one marked for destruction fails.
                    if super::model_defines_instance_method(model, &Symbol::from(format!("__nested_{}_marked", name.as_str()))) {
                        check = skip_when_nested_pending(check, name);
                    }
                    check.inherit_span(span);
                    stmts.push(check);
                }
            }
            _ => {}
        }
    }

    stmts.extend(secure_password_checks(model));

    // An ActiveModel class with no rule still has `errors`: `errors.add`
    // in its own methods needs the same surface a validating one gets.
    let active_model = model.parent.is_none() && includes_active_model_validations(model);
    if stmts.is_empty() && !active_model {
        return;
    }

    // Empty synth must not shadow a source `def validate`:
    // `push_user_methods` keeps Method synthesizers over source defs, so
    // pushing here would drop the user's body on the newly opened
    // ruleless ActiveModel path. Still emit the surface below.
    let source_validate =
        super::model_defines_instance_method(model, &Symbol::from("validate"));
    if !(stmts.is_empty() && source_validate) {
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from("validate"),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: seq(stmts),
            signature: Some(fn_sig(vec![], Ty::Nil)),
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }

    // A validating model OUTSIDE the ActiveRecord hierarchy (`class
    // Search` + `include ActiveModel::Validations`, no superclass)
    // can't inherit `valid?`/`errors` from the runtime Base —
    // synthesize the same pair here. Skipped when the source defines
    // either name itself.
    if model.parent.is_none() {
        push_active_model_validation_surface(methods, model, errors_readers);
    }
}

/// The validations `has_secure_password` adds unless it is written
/// with `validations: false` — the ones that make a password reset with
/// a mismatched confirmation FAIL, as the Rails 8 authentication
/// generator's PasswordsController and its test rely on:
///
///   errors << "Password can't be blank" if @password_digest blank
///   errors << "Password is too long" if @password.bytesize > 72
///   errors << "Password confirmation doesn't match Password"
///     if @password_confirmation given and != @password (allow_blank)
///
/// The plaintext ivars are the ones `lower::secure_password` writes.
fn secure_password_checks(model: &Model) -> Vec<Expr> {
    if !secure_password_validates(model) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for attr in crate::lower::secure_password::secure_password_attrs(&model.body) {
        let human = model.human_attribute_name(attr.as_str());
        let msgs = Messages(Some(model), None);
        let plain = ivar(&attr);
        let digest = ivar(&Symbol::from(format!("{}_digest", attr.as_str())));
        let confirmation = ivar(&Symbol::from(format!("{}_confirmation", attr.as_str())));
        let blank = |e: Expr| bool_op(BoolOpKind::Or, send(e.clone(), "nil?", vec![]), send(e, "empty?", vec![]));
        let not = |e: Expr| Expr::new(
            Span::synthetic(),
            ExprNode::Send { recv: Some(e), method: Symbol::from("!"), args: vec![], block: None, parenthesized: false },
        );
        let present = |e: Expr| not(blank(e));
        out.push(if_with_nil_else(blank(digest), errors_push(attr.as_str(), msgs.error(attr.as_str(), "blank", &Default::default()))));
        out.push(if_with_nil_else(
            bool_op(
                BoolOpKind::And,
                present(plain.clone()),
                send(send(plain.clone(), "bytesize", vec![]), ">", vec![Expr::new(
                    Span::synthetic(),
                    ExprNode::Lit { value: Literal::Int { value: 72 } },
                )]),
            ),
            errors_push(attr.as_str(), msgs.error(attr.as_str(), "password_too_long", &Default::default())),
        ));
        out.push(if_with_nil_else(
            bool_op(
                BoolOpKind::And,
                bool_op(BoolOpKind::And, present(plain.clone()), not(send(confirmation.clone(), "nil?", vec![]))),
                send(confirmation, "!=", vec![plain]),
            ),
            errors_push(&format!("{}_confirmation", attr.as_str()), msgs.error(
                &format!("{}_confirmation", attr.as_str()),
                "confirmation",
                &ErrorOpts { attribute: Some(human.clone()), ..Default::default() },
            )),
        ));
    }
    out
}

/// `has_secure_password` without `validations: false`.
fn secure_password_validates(model: &Model) -> bool {
    use crate::dialect::ModelBodyItem;
    let mut declared = false;
    for item in &model.body {
        let ModelBodyItem::Unknown { expr, .. } = item else { continue };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { continue };
        if method.as_str() != "has_secure_password" {
            continue;
        }
        declared = true;
        let off = args.iter().any(|a| matches!(&*a.node, ExprNode::Hash { entries, .. }
            if entries.iter().any(|(k, v)|
                matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "validations")
                    && matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: false } }))));
        if off {
            return false;
        }
    }
    declared
}

/// `ActiveModel::Model`'s attribute-hash constructor —
/// `Metadata.new(title: …, url: …)`, which the module supplies and
/// `ActiveModel::Validations` alone does NOT. campfire's
/// `Opengraph::Metadata` is built exactly once, by
/// `new attributes.merge(…)` in its own `from_url`, and without this
/// the call was "wrong number of arguments (given 1, expected 0)" —
/// the class had only Object's zero-arg `initialize`.
///
/// Assigns the attributes the model declares through `attr_*`, which is
/// what the module does: unknown keys raise `UnknownAttributeError` in
/// Rails, and dropping them here is the same shape as the permitted-
/// field ledger elsewhere rather than a silent write to nowhere.
///
/// Skipped when the source writes its own `initialize` — campfire's
/// `Opengraph::Location` does, and its one positional arg is the whole
/// point of that class.
pub(super) fn push_active_model_constructor(methods: &mut Vec<MethodDef>, model: &Model) {
    if !includes_active_model_model(model) {
        return;
    }
    let defines_initialize = model.body.iter().any(|item| matches!(
        item,
        crate::dialect::ModelBodyItem::Method { method, .. }
            if method.name.as_str() == "initialize"
    )) || methods.iter().any(|m| m.name.as_str() == "initialize");
    if defines_initialize {
        return;
    }
    let mut names = super::markers::declared_attr_names(model);
    // `attribute :name, :type` (ActiveModel::Attributes) names, assigned
    // through their casting writers.
    let attribute_api: Vec<Symbol> =
        super::markers::attribute_api_decls(&model.body).into_iter().map(|(name, _)| name).collect();
    for name in &attribute_api {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    if names.is_empty() {
        return;
    }
    // ONE definition of what the module's constructor is — the
    // library-class twin (`lower::active_model_model`) covers the
    // classes ingest cannot classify as tableless models (campfire's
    // `ActionText::Attachment::OpengraphEmbed`, which lives in `lib/`),
    // and the two must not drift into two constructors.
    let defaulted: Vec<Symbol> =
        super::markers::attribute_api_defaults(&model.body).into_iter().map(|(name, _)| name).collect();
    methods.push(crate::lower::active_model_model::attributes_initialize(
        &model.name,
        model.span,
        &names,
        &attribute_api,
        &defaulted,
    ));
}

/// Bare `include ActiveModel::<leaf>` in the class body for any of `modules`.
fn includes_active_model(model: &Model, modules: &[&str]) -> bool {
    model.body.iter().any(|item| {
        let crate::dialect::ModelBodyItem::Unknown { expr, .. } = item else { return false };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else {
            return false;
        };
        method.as_str() == "include"
            && args.iter().any(|a| matches!(&*a.node,
                ExprNode::Const { path } if path.len() == 2
                    && path[0].as_str() == "ActiveModel"
                    && modules.contains(&path[1].as_str())))
    })
}

/// `include ActiveModel::Validations` or `include ActiveModel::Model`
/// (which includes it) in the class body.
fn includes_active_model_validations(model: &Model) -> bool {
    includes_active_model(model, &["Validations", "Model"])
}

/// `include ActiveModel::Model` in the class body. The narrower
/// `ActiveModel::Validations` brings `valid?`/`errors` and no
/// constructor, so the two are not interchangeable here.
fn includes_active_model_model(model: &Model) -> bool {
    includes_active_model(model, &["Model"])
}

/// `before_validation :sanitize_fields` in the model body. Only the
/// method-name form — a block callback (`before_validation do … end`)
/// is synthesized under the same method name, so it is included too;
/// anything `push_callback_methods` declines to synthesize would leave
/// this call dangling, which is why the two read the same declaration.
fn declares_before_validation(model: &Model) -> bool {
    model.body.iter().any(|item| {
        matches!(item, crate::dialect::ModelBodyItem::Callback { callback, .. }
            if callback.hook == crate::dialect::CallbackHook::BeforeValidation)
    })
}

/// `valid?` + `errors` mirroring the runtime Base implementations,
/// for `include ActiveModel::Validations` classes with no superclass.
fn push_active_model_validation_surface(methods: &mut Vec<MethodDef>, model: &Model, errors_readers: ErrorsReaders) {
    let defines = |name: &str| {
        model.body.iter().any(|item| matches!(
            item,
            crate::dialect::ModelBodyItem::Method { method, .. } if method.name.as_str() == name
        ))
    };
    let span = model.span;
    let errors_read = || Expr::new(span, ExprNode::Ivar { name: Symbol::from("errors") });
    if !defines("valid?") {
        // `before_validation :sanitize_fields` runs INSIDE `valid?` for
        // an ActiveModel class (`ActiveModel::Validations::Callbacks`),
        // not on save — there is no save. The callback method is
        // synthesized by `push_callback_methods`, and nothing called
        // it: campfire's `Opengraph::Metadata` sanitizes its title and
        // description there, so the raw `<script>` survived into the
        // record and three tests read it back.
        //
        // Ordered first, which is what the name says and what the
        // sanitize case needs — the presence checks below must see the
        // sanitized values.
        let mut body_stmts: Vec<Expr> = Vec::new();
        // Asked of the model BODY, not of `methods`: this pass runs
        // before `push_callback_methods` synthesizes the callback (847
        // vs 890 in mod.rs), so the method does not exist yet — and
        // reordering the two to suit this read would be the tail
        // wagging the dog.
        if declares_before_validation(model) {
            body_stmts.push(Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("before_validation"),
                    args: Vec::new(),
                    block: None,
                    parenthesized: false,
                },
            ));
        }
        let reset = |name: &str| {
            Expr::new(
                span,
                ExprNode::Assign {
                    target: crate::expr::LValue::Ivar { name: Symbol::from(name) },
                    value: Expr::new(
                        span,
                        ExprNode::Array { elements: vec![], style: ArrayStyle::default() },
                    ),
                },
            )
        };
        body_stmts.extend(vec![
            reset("errors"),
            reset("error_details"),
            Expr::new(
                span,
                ExprNode::Send {
                    recv: None,
                    method: Symbol::from("validate"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
            Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(errors_read()),
                    method: Symbol::from("empty?"),
                    args: vec![],
                    block: None,
                    parenthesized: false,
                },
            ),
        ]);
        let body = seq(body_stmts);
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from("valid?"),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body,
            signature: Some(fn_sig(vec![], Ty::Bool)),
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: true,
            block_param: None,
        });
    }
    if !defines("errors") {
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from("errors"),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: errors_reader(span, errors_readers, "errors", Ty::Array { elem: std::sync::Arc::new(Ty::Str) }),
            signature: Some(fn_sig(vec![], Ty::Array { elem: std::sync::Arc::new(Ty::Str) })),
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
    if !defines("error_details") {
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from("error_details"),
            receiver: MethodReceiver::Instance,
            params: Vec::new(),
            body: errors_reader(span, errors_readers, "error_details", crate::lower::errors_add::error_details_ty()),
            signature: Some(fn_sig(vec![], crate::lower::errors_add::error_details_ty())),
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }    if !defines("error_messages_for") {
        methods.push(MethodDef {
            visibility: crate::dialect::MethodVisibility::Public,
            unsupported_formals: None,
            has_anonymous_block: false,
            name_span: crate::span::Span::synthetic(),
            name: Symbol::from("error_messages_for"),
            receiver: MethodReceiver::Instance,
            params: vec![crate::dialect::Param::positional(Symbol::from("attribute"))],
            body: error_messages_for_body(span, errors_readers),
            signature: Some(fn_sig(vec![(Symbol::from("attribute"), Ty::Str)], Ty::Array { elem: std::sync::Arc::new(Ty::Str) })),
            effects: EffectSet::default(),
            enclosing_class: Some(model.name.0.clone()),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        });
    }
}

/// `@<name> ||= []`, created on first read as Rails' `errors` is, so an
/// `errors.add` before any `valid?` has a list to add to; `@<name>` for
/// `ErrorsReaders::FieldRead`.
fn errors_reader(span: Span, errors_readers: ErrorsReaders, name: &str, ty: Ty) -> Expr {
    if errors_readers == ErrorsReaders::FieldRead {
        return Expr::new(span, ExprNode::Ivar { name: Symbol::from(name) });
    }
    let mut empty = Expr::new(span, ExprNode::Array { elements: vec![], style: ArrayStyle::default() });
    empty.ty = Some(ty.clone());
    let mut read = Expr::new(
        span,
        ExprNode::OpAssign {
            target: crate::expr::LValue::Ivar { name: Symbol::from(name) },
            op: crate::expr::OpAssignOp::OrOr,
            value: empty,
        },
    );
    read.ty = Some(ty);
    read
}

/// `out = []; error_details.each { |e| out << e.message if e.attribute == attribute }; out`,
/// the body `ActiveRecord::Base#error_messages_for` has in the shared runtime
/// (`@error_details.each` for `ErrorsReaders::FieldRead`).
fn error_messages_for_body(span: Span, errors_readers: ErrorsReaders) -> Expr {
    let str_array = Ty::Array { elem: std::sync::Arc::new(Ty::Str) };
    let typed = |node: ExprNode, ty: Ty| {
        let mut e = Expr::new(span, node);
        e.ty = Some(ty);
        e
    };
    let var = |name: &str, ty: Ty| typed(ExprNode::Var { id: crate::ident::VarId(0), name: Symbol::from(name) }, ty);
    let call = |recv: Expr, method: &str, args: Vec<Expr>, ty: Ty| {
        typed(ExprNode::Send { recv: Some(recv), method: Symbol::from(method), args, block: None, parenthesized: false }, ty)
    };
    let error_ty = Ty::Class { id: ClassId(Symbol::from("ActiveRecord::ErrorDetail")), args: vec![].into() };
    let error = || var("e", error_ty.clone());
    let picked = typed(
        ExprNode::If {
            cond: call(call(error(), "attribute", vec![], Ty::Str), "==", vec![var("attribute", Ty::Str)], Ty::Bool),
            then_branch: call(var("out", str_array.clone()), "<<", vec![call(error(), "message", vec![], Ty::Str)], str_array.clone()),
            else_branch: typed(ExprNode::Lit { value: Literal::Nil }, Ty::Nil),
        },
        Ty::Nil,
    );
    let block = Expr::new(
        span,
        ExprNode::Lambda {
            extra_params: Vec::new(),
            rest_param: None,
            params: vec![Symbol::from("e")],
            block_param: None,
            body: picked,
            block_style: crate::expr::BlockStyle::Brace,
        },
    );
    let details_read = match errors_readers {
        ErrorsReaders::FieldRead => ExprNode::Ivar { name: Symbol::from("error_details") },
        ErrorsReaders::CreatedOnFirstRead => {
            ExprNode::Send { recv: None, method: Symbol::from("error_details"), args: vec![], block: None, parenthesized: false }
        }
    };
    let details = typed(details_read, crate::lower::errors_add::error_details_ty());
    let each = typed(
        ExprNode::Send { recv: Some(details), method: Symbol::from("each"), args: vec![], block: Some(block), parenthesized: false },
        crate::lower::errors_add::error_details_ty(),
    );
    let init = Expr::new(
        span,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { id: crate::ident::VarId(0), name: Symbol::from("out") },
            value: typed(ExprNode::Array { elements: vec![], style: ArrayStyle::default() }, str_array.clone()),
        },
    );
    seq(vec![init, each, var("out", str_array)])
}

/// Produce the list of helper-call expressions for one `ValidationRule` on
/// `attr`. Each helper is `<helper>(:attr, @attr [, kwargs])` — the value
/// is passed positionally so the runtime helper sees a concretely-typed
/// `value` parameter (no block-yield, no `instance_variable_get`).
fn validation_rule_to_calls(
    msgs: Messages,
    attr: &Symbol,
    rule: &ValidationRule,
    attr_ty: Option<&Ty>,
    runtime_relation: super::RuntimeRelation,
) -> Vec<Expr> {
    match rule {
        ValidationRule::Presence => vec![inline_presence_check(msgs, attr, attr_ty, runtime_relation)],
        ValidationRule::Absence => vec![inline_absence_check(msgs, attr, attr_ty, runtime_relation)],
        ValidationRule::Acceptance { accept, allow_nil } => {
            vec![inline_acceptance_check(msgs, attr, accept, *allow_nil != Some(false), attr_ty, runtime_relation)]
        }
        ValidationRule::Length { min, max, message, allow_nil } => inline_length_check(
            msgs,
            attr,
            min.map(|n| n as usize),
            max.map(|n| n as usize),
            message.as_deref(),
            attr_ty,
            *allow_nil,
        ),
        ValidationRule::Format { pattern, flags, allow_nil, allow_blank } => skip_nil_or_blank(
            vec![inline_format_check(
                msgs,
                attr,
                pattern,
                flags,
                attr_ty,
                *allow_nil == Some(true) || *allow_blank == Some(true),
                runtime_relation,
            )],
            attr,
            attr_ty,
            *allow_nil,
            *allow_blank,
            runtime_relation,
        ),
        ValidationRule::Numericality { only_integer, gt, lt, gte, lte, allow_nil, allow_blank, in_range } => skip_nil_or_blank(
            inline_numericality_check(msgs, attr, *only_integer, *gt, *lt, *gte, *lte, in_range.as_ref()),
            attr,
            attr_ty,
            *allow_nil,
            *allow_blank,
            runtime_relation,
        ),
        ValidationRule::Inclusion { values, range } if range.is_none() && is_array_ty(attr_ty) => {
            vec![inline_set_call_check(msgs, attr, &literal_list(values), false, attr_ty)]
        }
        ValidationRule::Exclusion { values, range } if range.is_none() && is_array_ty(attr_ty) => {
            vec![inline_set_call_check(msgs, attr, &literal_list(values), true, attr_ty)]
        }
        ValidationRule::Inclusion { values, range } => {
            vec![inline_inclusion_check(msgs, attr, values, range.as_ref())]
        }
        ValidationRule::Exclusion { values, range } => {
            vec![inline_exclusion_check(msgs, attr, values, range.as_ref())]
        }
        ValidationRule::InclusionCall { set } => vec![inline_set_call_check(msgs, attr, set, false, attr_ty)],
        ValidationRule::ExclusionCall { set } => vec![inline_set_call_check(msgs, attr, set, true, attr_ty)],
        // `validate :validate_url` — a method the model defines, which
        // adds to `errors` itself. The synthesized `validate` just
        // calls it; everything else about the shape (when it runs, what
        // `valid?` does with `errors` afterwards) is already the same
        // for a rule-derived check.
        ValidationRule::Custom { method, if_method, unless_method } => {
            let bare = |m: &Symbol| {
                Expr::new(
                    Span::synthetic(),
                    ExprNode::Send {
                        recv: None,
                        method: m.clone(),
                        args: Vec::new(),
                        block: None,
                        parenthesized: false,
                    },
                )
            };
            // `if: :pred` / `unless: :pred` guard the call, as Rails'
            // callback conditions do.
            vec![with_method_guard(bare(method), if_method, unless_method)]
        }
        ValidationRule::Uniqueness { .. } => {
            // Not yet exercised by real-blog; lands when a fixture forces the issue.
            Vec::new()
        }
    }
}

/// Inline `belongs_to` presence check (Rails 5+ default — every
/// non-optional `belongs_to` requires the associated record to
/// exist). Generates the IR equivalent of:
///   if @article_id.nil? || @article_id == 0 || article.nil?
///     errors << "article must exist"
///   end
/// THROUGH THE READER, as Rails' `validates_presence_of :article`
/// (what `belongs_to` declares, `message: :required`) reads the
/// association: a target already loaded or assigned (`message.room =
/// room`, `creator: Current.user`) costs no query, and one that is not
/// is loaded once and kept for the save's later reads (a `touch: true`
/// cascade). The reader goes by the association's `primary_key:`, and
/// a foreign-key write clears what it holds, so a reassigned key is
/// checked afresh. An `exists?` probe here asked the database again
/// for every save, inside the save's transaction, which holds SQLite's
/// one write lock while it runs.
///
/// A POLYMORPHIC association keeps only the first two terms. The class
/// to query is in the row's `<assoc>_type` COLUMN, and `target` for a
/// polymorphic belongs_to is the assoc-name phantom (`Record` for
/// `belongs_to :record`), a class that does not exist and has no typed
/// reader. Dropping the existence half is a strict SUBSET of Rails'
/// check — it can fail a save Rails would fail, never one Rails would
/// pass — and the id-presence half is the part that catches the real
/// error (an unset owner).
/// Inline `validates :attr, uniqueness: …`, as Rails 8.1's
/// `UniquenessValidator` queries (on a target with the runtime Relation):
///   if Model.unscoped.where(attr: @attr).not(<pk>: @<pk>)[.where(scope_col: @scope_col)…].exists?
///     errors << "attr has already been taken"
///   end
/// `case_sensitive: false` compares `LOWER(table.attr) = LOWER(?)`, except
/// for a nil value, which Rails compares `IS NULL` either way. A nil value
/// is checked like any other (two nils collide) unless `allow_nil:`, and a
/// blank one (nil, or empty once stripped) unless `allow_blank:`. The `not(<pk>: @<pk>)` is Rails'
/// persisted-record exclusion, by the model's declared primary key (`"codes"."code" != ?`
/// under `self.primary_key = "code"`, even beside an `id` column); on a new record it is `<pk> IS NOT NULL`,
/// which every stored row satisfies, so the answer is the same. A `scope:`
/// naming a `belongs_to` reads its foreign key (and type column when
/// polymorphic), as Rails resolves it, and so does the validated attribute
/// itself when it names a non-polymorphic `belongs_to` (`validates
/// :reblog, uniqueness:` queries `reblog_of_id`; the error stays on
/// `reblog`).
fn inline_uniqueness_check(
    msgs: Messages,
    model: &Model,
    primary_key: &Symbol,
    attr: &Symbol,
    scope: &[Symbol],
    case_sensitive: bool,
    allow_nil: bool,
    allow_blank: bool,
) -> Expr {
    let sym = |name: &str| {
        Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Sym { value: Symbol::from(name) } })
    };
    let hash = |key: &str, value: Expr| {
        Expr::new(Span::synthetic(), ExprNode::Hash { entries: vec![(sym(key), value)], kwargs: true })
    };
    let finder = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: model.name.0.as_str().split("::").map(Symbol::from).collect() },
    );
    // `Model.unscoped`, as Rails' UniquenessValidator builds its query on
    // `finder_class.unscoped`: a row a `default_scope` hides still collides.
    let finder = send(finder, "unscoped", vec![]);
    // An attribute naming a `belongs_to` compares its foreign key, as
    // Rails' `bind_attribute` resolves it (`where(reblog: r)` is
    // `"reblog_of_id" = r.id`), and the value read is that key. Ingest
    // keeps a polymorphic one a gap: Rails raises computing its class.
    let column = model
        .associations()
        .find_map(|a| match a {
            Association::BelongsTo { name, foreign_key, polymorphic: false, .. } if name == attr => Some(foreign_key.clone()),
            _ => None,
        })
        .unwrap_or_else(|| attr.clone());
    let association = column != *attr;
    let value = ivar(&column);
    // `case_sensitive: false` lowers a String comparison, a String foreign
    // key's too (Rails: `LOWER("tickets"."code_ref") = LOWER(?)`). An
    // Integer key compares as an id: Rails' `LOWER` over it answers the same.
    let string_key = match model.attributes.fields.get(&column) {
        Some(Ty::Str) => true,
        Some(Ty::Union { variants }) => variants.iter().any(|v| matches!(v, Ty::Str)),
        _ => false,
    };
    let mut relation = if case_sensitive || (association && !string_key) {
        send(finder, "where", vec![hash(column.as_str(), value.clone())])
    } else {
        let lowered = Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Str {
                    value: format!("LOWER({}.{}) = LOWER(?)", model.table.0.as_str(), column.as_str()),
                },
            },
        );
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: send(value.clone(), "nil?", vec![]),
                then_branch: send(finder.clone(), "where", vec![hash(column.as_str(), value.clone())]),
                else_branch: send(finder, "where", vec![lowered, value.clone()]),
            },
        )
    };
    relation = send(relation, "not", vec![hash(primary_key.as_str(), ivar(primary_key))]);
    for name in scope {
        let columns: Vec<Symbol> = match model.associations().find(|a| {
            matches!(a, Association::BelongsTo { name: n, .. } if n == name)
        }) {
            Some(Association::BelongsTo { foreign_key, polymorphic, .. }) => {
                let mut cols = vec![foreign_key.clone()];
                if *polymorphic {
                    cols.push(Symbol::from(format!("{}_type", name.as_str()).as_str()));
                }
                cols
            }
            _ => vec![name.clone()],
        };
        for column in columns {
            relation = send(relation, "where", vec![hash(column.as_str(), ivar(&column))]);
        }
    }
    let mut taken = send(relation, "exists?", vec![]);
    let not = |e: Expr| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send { recv: Some(e), method: Symbol::from("!"), args: vec![], block: None, parenthesized: false },
        )
    };
    if allow_blank {
        // `blank?` spelled out (`nil?` or empty once stripped): the Spinel
        // runtime's String has no ActiveSupport `blank?`.
        let blank = bool_op(
            BoolOpKind::Or,
            send(value.clone(), "nil?", vec![]),
            send(send(send(value.clone(), "to_s", vec![]), "strip", vec![]), "empty?", vec![]),
        );
        taken = bool_op(BoolOpKind::And, not(blank), taken);
    } else if allow_nil {
        taken = bool_op(BoolOpKind::And, not(send(value.clone(), "nil?", vec![])), taken);
    }
    let push_err = errors_push(attr.as_str(), msgs.error(attr.as_str(), "taken", &Default::default()));
    if_with_nil_else(taken, push_err)
}

fn inline_belongs_to_check(
    msgs: Messages,
    assoc_name: &Symbol,
    foreign_key: &Symbol,
    polymorphic: bool,
) -> Expr {
    let fk_ivar = ivar(foreign_key);
    // `@fk.nil?`
    let nil_check = send(fk_ivar.clone(), "nil?", vec![]);
    // `@fk == 0`
    let zero_check = send(
        fk_ivar,
        "==",
        vec![Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Int { value: 0 } },
        )],
    );
    // Not a `:required` error: Rails' `belongs_to` validates presence with `message: :required`, so the type stays `:blank`.
    let mut message = msgs.error(assoc_name.as_str(), "required", &Default::default());
    message.kind = "blank".to_string();
    let push_err = errors_push(assoc_name.as_str(), message);
    if polymorphic {
        return if_with_nil_else(bool_op(BoolOpKind::Or, nil_check, zero_check), push_err);
    }
    // `article.nil?` — the association reader, on self.
    let reader = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: assoc_name.clone(),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let missing = send(reader, "nil?", vec![]);
    let cond = bool_op(
        BoolOpKind::Or,
        bool_op(BoolOpKind::Or, nil_check, zero_check),
        missing,
    );
    if_with_nil_else(cond, push_err)
}

/// `if <cond> … end` from `inline_belongs_to_check`, with `&&
/// (@__nested_<assoc>.nil? || @__nested_<assoc>.empty?)` added: no record
/// built or updated by the association's nested-attributes writer is
/// waiting for the save. A record the writer marked for destruction
/// counts as absent, as Rails' presence check reads it: the check also
/// fails when the generated `__nested_<assoc>_marked` answers true.
fn skip_when_nested_pending(check: Expr, assoc_name: &Symbol) -> Expr {
    let ExprNode::If { cond, then_branch, else_branch } = &*check.node else { return check };
    let pending = ivar(&Symbol::from(format!("__nested_{}", assoc_name.as_str())));
    let none_pending = bool_op(
        BoolOpKind::Or,
        send(pending.clone(), "nil?", vec![]),
        send(pending, "empty?", vec![]),
    );
    let marked = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from(format!("__nested_{}_marked", assoc_name.as_str())),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let missing = bool_op(BoolOpKind::Or, cond.clone(), marked);
    Expr::new(
        check.span,
        ExprNode::If {
            cond: bool_op(BoolOpKind::And, missing, none_pending),
            then_branch: then_branch.clone(),
            else_branch: else_branch.clone(),
        },
    )
}

/// `validate :method, if: :pred` / `unless: :pred`. The Symbol becomes
/// the same receiverless call `validates`'s expression guard uses.
fn with_method_guard(body: Expr, if_method: &Option<Symbol>, unless_method: &Option<Symbol>) -> Expr {
    let bare = |m: &Symbol| {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: m.clone(),
                args: Vec::new(),
                block: None,
                parenthesized: false,
            },
        )
    };
    with_condition_guard(body, &if_method.as_ref().map(bare), &unless_method.as_ref().map(bare))
}

/// `if:` / `unless:` on `validates`. Both given: the check runs only
/// when `if:` holds and `unless:` does not — the `unless` guard nests
/// inside the `if`.
fn with_condition_guard(body: Expr, if_cond: &Option<Expr>, unless_cond: &Option<Expr>) -> Expr {
    let nil = || Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil });
    let if_expr = if_cond.clone();
    let unless_expr = unless_cond.clone();
    let guarded = match unless_expr {
        Some(cond) => Expr::new(
            Span::synthetic(),
            ExprNode::If { cond, then_branch: nil(), else_branch: body },
        ),
        None => body,
    };
    match if_expr {
        Some(cond) => Expr::new(
            Span::synthetic(),
            ExprNode::If { cond, then_branch: guarded, else_branch: nil() },
        ),
        None => guarded,
    }
}

/// `@<attr>` — direct ivar read passed as the `value` positional arg
/// to every validates_* helper.
fn ivar(attr: &Symbol) -> Expr {
    Expr::new(Span::synthetic(), ExprNode::Ivar { name: attr.clone() })
}

/// Inline `validates :attr, presence: true` expansion.
/// Generates the IR equivalent of:
///   if @attr.nil? || (@attr.is_a?(String) && @attr.empty?) || (@attr.is_a?(Array) && @attr.empty?)
///     errors << "attr can't be blank"
///   end
///
/// Mirrors `runtime/ruby/active_record/validations.rb::validates_presence_of`
/// exactly; no behavior change vs. the helper-call form, but the
/// expansion lives in the IR so every target emits inline checks
/// instead of routing through the Validations module at runtime.
/// When `attr_ty` is `Some(Ty::Str)` the `is_a?(Array)` arm drops out
/// (typed targets like TS narrow `Array.isArray(stringField)` to
/// `never` and reject the subsequent property access). Same logic for
/// `Some(Ty::Array { .. })` — the String arm drops. `None` keeps the
/// generic three-way form so untyped/dynamic-shape attrs still work.
fn inline_presence_check(msgs: Messages, attr: &Symbol, attr_ty: Option<&Ty>, runtime_relation: super::RuntimeRelation) -> Expr {
    let cond = blank_cond(attr, attr_ty, runtime_relation);
    // `errors << "attr can't be blank"`
    let push_err = errors_push(attr.as_str(), msgs.error(attr.as_str(), "blank", &Default::default()));
    // The wrapping `if cond then push_err end` (Nil else).
    if_with_nil_else(cond, push_err)
}

/// Rails' `blank?` for a presence or absence check: nil, `false`, a String
/// of nothing but Unicode whitespace (`/\A[[:space:]]*\z/`: U+3000 and
/// U+00A0 count, U+200B does not), an empty Array or Hash.
///
/// The ruby family can hold the raw input in any column (the writer does not
/// cast), so every column but a String, Array or temporal one gets the full
/// test, and a String column the whitespace regex over its value's `to_s`
/// (Rails casts a non-String value there first, and no cast is blank). A strict target's field is
/// its column's type, so it gets only what that type can be: a String's
/// `strip.empty?` (each target's trim: Unicode White_Space on rust and go;
/// TypeScript's `trim()` keeps U+0085 and removes U+FEFF, the two places
/// it differs from Rails' `blank?`),
/// a boolean's `== false`, an Array's `empty?`, and `nil?` alone for a
/// number. No `empty?` on a type that has none.
///
/// Temporal columns (a `Time`, `Date` or `ActiveSupport::Duration`
/// attribute, nullable or not) store their text in `@<attr>_raw`;
/// `@<attr>` is never the storage slot (the ruby tree's parse memo is
/// `@__t_<attr>`), so a check against it fired unconditionally —
/// lobsters' `validates :created_at, presence: true` on Username
/// rejected every record. Blank on the stored form is nil-or-empty text.
fn blank_cond(attr: &Symbol, attr_ty: Option<&Ty>, runtime_relation: super::RuntimeRelation) -> Expr {
    let temporal = match peel_nilable(attr_ty).0 {
        Some(Ty::Time | Ty::Date) => true,
        Some(Ty::Class { id, .. }) => id.0.as_str() == "ActiveSupport::Duration",
        _ => false,
    };
    if temporal {
        let raw_ivar = ivar(&Symbol::from(format!("{}_raw", attr.as_str())));
        return bool_op(
            BoolOpKind::Or,
            send(raw_ivar.clone(), "nil?", vec![]),
            send(raw_ivar, "empty?", vec![]),
        );
    }
    let attr_ivar = ivar(attr);
    let nil_check = send(attr_ivar.clone(), "nil?", vec![]);
    // Nullable column: the `nil?` disjunct already covers nil, so the
    // emptiness test reads the narrowed value — `@attr.empty?` on an
    // Option doesn't compile on a strict target.
    let (attr_ty, nilable) = peel_nilable(attr_ty);
    let narrowed = match (nilable, attr_ty) {
        (true, Some(inner)) => narrowed_ivar(attr, inner),
        _ => attr_ivar.clone(),
    };
    let or = |a, b| bool_op(BoolOpKind::Or, a, b);
    let and = |a, b| bool_op(BoolOpKind::And, a, b);
    let is_false = |value: Expr| {
        send(value, "==", vec![Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: false } })])
    };
    match (runtime_relation, attr_ty) {
        (_, Some(Ty::Array { .. })) => or(nil_check, send(narrowed, "empty?", vec![])),
        // The uncast writer can hold an Integer, `false` or an Array here,
        // which Rails casts to a String before validating (`5` is `"5"`,
        // `false` is `"f"`, `[]` is `"[]"`): none is blank, and `to_s`
        // answers the same without raising on the regexp.
        (super::RuntimeRelation::Present, Some(Ty::Str)) => {
            or(nil_check, whitespace(send(attr_ivar.clone(), "to_s", vec![])))
        }
        (super::RuntimeRelation::Present, _) => or(
            or(
                or(or(nil_check, is_false(attr_ivar.clone())), and(is_a_check(&attr_ivar, "String"), whitespace(attr_ivar.clone()))),
                and(is_a_check(&attr_ivar, "Array"), send(attr_ivar.clone(), "empty?", vec![])),
            ),
            and(is_a_check(&attr_ivar, "Hash"), send(attr_ivar, "empty?", vec![])),
        ),
        (super::RuntimeRelation::Absent, Some(Ty::Str)) => {
            or(nil_check, send(stripped_str(narrowed), "empty?", vec![]))
        }
        (super::RuntimeRelation::Absent, Some(Ty::Bool)) => or(nil_check, is_false(narrowed)),
        (super::RuntimeRelation::Absent, Some(_)) => nil_check,
        // No type known: the old generic form, unchanged.
        (super::RuntimeRelation::Absent, None) => or(
            or(nil_check, and(is_a_check(&attr_ivar, "String"), send(attr_ivar.clone(), "empty?", vec![]))),
            and(is_a_check(&attr_ivar, "Array"), send(attr_ivar, "empty?", vec![])),
        ),
    }
}

/// Rails' `blank?` for a String, `/\A[[:space:]]*\z/.match?(value)`:
/// U+3000 and U+00A0 count as whitespace, U+200B does not.
fn whitespace(value: Expr) -> Expr {
    let re = Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Regex { pattern: r"\A[[:space:]]*\z".to_string(), flags: String::new() } },
    );
    send(re, "match?", vec![value])
}

/// Rails skips an `EachValidator` for a nil value under `allow_nil:` and a
/// blank one under `allow_blank:` (`EachValidator#validate`): the checks
/// run in the `else` of `if <skip> then nil`.
fn skip_nil_or_blank(
    checks: Vec<Expr>,
    attr: &Symbol,
    attr_ty: Option<&Ty>,
    allow_nil: Option<bool>,
    allow_blank: Option<bool>,
    runtime_relation: super::RuntimeRelation,
) -> Vec<Expr> {
    let cond = if allow_blank == Some(true) {
        allow_blank_cond(attr, attr_ty, runtime_relation)
    } else if allow_nil == Some(true) {
        send(ivar(attr), "nil?", vec![])
    } else {
        return checks;
    };
    vec![Expr::new(
        Span::synthetic(),
        ExprNode::If {
            cond,
            then_branch: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Nil }),
            else_branch: seq(checks),
        },
    )]
}

/// `allow_blank:`'s skip for a numericality or format rule. On the ruby
/// family the uncast writer can store the raw input, so a whitespace-only
/// String (and `false`) is blank, as Rails reads it: `@attr.nil? ||
/// @attr == false || /\A[[:space:]]*\z/.match?(@attr.to_s)` (`to_s`, the
/// spelling Spinel takes; the presence check's whitespace test). A String column drops the `false` disjunct: Rails casts `false`
/// to `"f"` there, which is not blank. A strict target's field is its
/// column's type: a String's narrowed `strip.empty?`, a Boolean's
/// narrowed `== false` (`false.blank?` is true), else `nil?` (a numeric
/// field is never a String or `false`), which every target compiles.
/// Narrower than the presence check's test, which is unchanged.
fn allow_blank_cond(attr: &Symbol, attr_ty: Option<&Ty>, runtime_relation: super::RuntimeRelation) -> Expr {
    let value = ivar(attr);
    let nil = send(value.clone(), "nil?", vec![]);
    let (attr_ty, nilable) = peel_nilable(attr_ty);
    let string_column = matches!(attr_ty, Some(Ty::Str));
    match runtime_relation {
        super::RuntimeRelation::Present => {
            let empty = whitespace(send(value.clone(), "to_s", vec![]));
            if string_column {
                return bool_op(BoolOpKind::Or, nil, empty);
            }
            let falsy = send(
                value,
                "==",
                vec![Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: false } })],
            );
            bool_op(BoolOpKind::Or, bool_op(BoolOpKind::Or, nil, falsy), empty)
        }
        super::RuntimeRelation::Absent if string_column => {
            // The `nil?` disjunct covers nil, so the strip reads the
            // narrowed value, as the presence check's does.
            let narrowed = if nilable { narrowed_ivar(attr, &Ty::Str) } else { value };
            bool_op(BoolOpKind::Or, nil, send(stripped_str(narrowed), "empty?", vec![]))
        }
        super::RuntimeRelation::Absent if matches!(attr_ty, Some(Ty::Bool)) => {
            let narrowed = if nilable { narrowed_ivar(attr, &Ty::Bool) } else { value };
            let falsy = send(
                narrowed,
                "==",
                vec![Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: false } })],
            );
            bool_op(BoolOpKind::Or, nil, falsy)
        }
        super::RuntimeRelation::Absent => nil,
    }
}

/// Inline `validates :attr, absence: true` — the negation of presence.
///   if !(@attr.nil? || (@attr.is_a?(String) && @attr.empty?) || (@attr.is_a?(Array) && @attr.empty?))
///     errors << "attr must be blank"
///   end
/// Reuses the presence condition tree and wraps with unary `!`.
fn inline_absence_check(msgs: Messages, attr: &Symbol, attr_ty: Option<&Ty>, runtime_relation: super::RuntimeRelation) -> Expr {
    let not_blank = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(blank_cond(attr, attr_ty, runtime_relation)),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let push_err = errors_push(attr.as_str(), msgs.error(attr.as_str(), "present", &Default::default()));
    if_with_nil_else(not_blank, push_err)
}

/// Inline `validates :attr, inclusion: { in: [v1, v2, …] }`.
///   if ![v1, v2, …].include?(@attr)
///     errors << "attr is not included in the list"
///   end
/// The `within.nil?` guard from the runtime helper is unnecessary —
/// the list is a known literal at lower time.
fn inline_inclusion_check(
    msgs: Messages,
    attr: &Symbol,
    values: &[Literal],
    range: Option<&crate::dialect::InclusionRange>,
) -> Expr {
    if let Some(range) = range {
        // `nil` is not in the range. Numeric `Range#include?` is
        // `cover?`: `1..3` accepts `1.5`, and `1...3` rejects `3`.
        let attr_ivar = ivar(attr);
        let begin = Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Float { value: range.begin } },
        );
        let end = Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Float { value: range.end } },
        );
        let below = send(attr_ivar.clone(), "<", vec![begin]);
        let above_op = if range.exclusive { ">=" } else { ">" };
        let above = send(attr_ivar.clone(), above_op, vec![end]);
        let outside = bool_op(BoolOpKind::Or, below, above);
        // A non-numeric value makes `<` raise. `cover?` is false when
        // `<=>` is nil, and Rails records the inclusion error.
        let nil = send(attr_ivar.clone(), "nil?", vec![]);
        let not_numeric = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(is_a_check(&attr_ivar, "Numeric")),
                method: Symbol::from("!"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let push_err =
            errors_push(attr.as_str(), msgs.error(attr.as_str(), "inclusion", &Default::default()));
        let bad = bool_op(BoolOpKind::Or, nil, not_numeric);
        return if_with_nil_else(bool_op(BoolOpKind::Or, bad, outside), push_err);
    }
    let array_lit = Expr::new(
        Span::synthetic(),
        ExprNode::Array {
            elements: values
                .iter()
                .map(|lit| Expr::new(Span::synthetic(), ExprNode::Lit { value: lit.clone() }))
                .collect(),
            style: ArrayStyle::Brackets,
        },
    );
    let attr_ivar = ivar(attr);
    let include_call = send(array_lit, "include?", vec![attr_ivar]);
    let not_included = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(include_call),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let push_err =
        errors_push(attr.as_str(), msgs.error(attr.as_str(), "inclusion", &Default::default()));
    if_with_nil_else(not_included, push_err)
}

/// Inline `inclusion:` / `exclusion:` with a callable `in:`, the set read
/// on the record when it validates (Rails' `Clusivity#include?`):
///   if !(<set>).include?(@attr)      # inclusion: "is not included in the list"
///   if (<set>).include?(@attr)       # exclusion: "is reserved"
///     errors << …
///   end
///
/// An Array value (an array column the target stores as an Array) is in
/// the set when every element is, as `Clusivity#include?` checks it:
/// `@attr.all? { |v| (<set>).include?(v) }` (nil is asked of the set as
/// it is). So an exclusion rejects it
/// only when all of its elements are reserved, and `[]` is in every set.
/// The set is read once per element there, where Rails reads it once.
fn inline_set_call_check(msgs: Messages, attr: &Symbol, set: &Expr, exclusion: bool, attr_ty: Option<&Ty>) -> Expr {
    let include_call = if is_array_ty(attr_ty) {
        let elem = Symbol::from("v");
        let test = send(
            set.clone(),
            "include?",
            vec![Expr::new(Span::synthetic(), ExprNode::Var { id: crate::ident::VarId(0), name: elem.clone() })],
        );
        let block = Expr::new(
            Span::synthetic(),
            ExprNode::Lambda {
                extra_params: Vec::new(),
                rest_param: None,
                params: vec![elem],
                block_param: None,
                body: test,
                block_style: crate::expr::BlockStyle::Brace,
            },
        );
        let all = Expr::new(
            Span::synthetic(),
            ExprNode::Send { recv: Some(ivar(attr)), method: Symbol::from("all?"), args: vec![], block: Some(block), parenthesized: false },
        );
        // A nil column value isn't an Array: Rails asks the set for nil.
        Expr::new(
            Span::synthetic(),
            ExprNode::If {
                cond: send(ivar(attr), "nil?", vec![]),
                then_branch: send(set.clone(), "include?", vec![ivar(attr)]),
                else_branch: all,
            },
        )
    } else {
        send(set.clone(), "include?", vec![ivar(attr)])
    };
    let cond = if exclusion {
        include_call
    } else {
        Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(include_call),
                method: Symbol::from("!"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        )
    };
    let kind = if exclusion { "exclusion" } else { "inclusion" };
    let push_err = errors_push(attr.as_str(), msgs.error(attr.as_str(), kind, &Default::default()));
    if_with_nil_else(cond, push_err)
}

/// An Array slot, nullable or not (`ty_of_column_slot`'s stored array
/// column, `Array[T] | nil` when the column allows null).
fn is_array_ty(ty: Option<&Ty>) -> bool {
    match ty {
        Some(Ty::Array { .. }) => true,
        Some(Ty::Union { variants }) => {
            variants.iter().any(|v| matches!(v, Ty::Array { .. }))
                && variants.iter().all(|v| matches!(v, Ty::Array { .. } | Ty::Nil))
        }
        _ => false,
    }
}

/// `[v1, v2]`, a literal `in:` list as an expression.
fn literal_list(values: &[Literal]) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Array {
            elements: values.iter().map(|lit| Expr::new(Span::synthetic(), ExprNode::Lit { value: lit.clone() })).collect(),
            style: ArrayStyle::Brackets,
        },
    )
}

/// Inline `validates :attr, exclusion: { in: … }`, Rails' "is reserved":
///   if [v1, v2].include?(@attr)            # a value list
///   if @attr.is_a?(Numeric) && @attr >= begin && @attr <= end   # a range
///     errors << "attr is reserved"
///   end
/// `nil` is reserved only when the list names it (`in: [nil, …]`), as
/// Rails' `ExclusionValidator` has no implicit `allow_nil`; a numeric
/// range's `cover?` is false for it (and for any non-numeric value).
fn inline_exclusion_check(
    msgs: Messages,
    attr: &Symbol,
    values: &[Literal],
    range: Option<&crate::dialect::InclusionRange>,
) -> Expr {
    let attr_ivar = ivar(attr);
    let reserved = if let Some(range) = range {
        let begin = Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Float { value: range.begin } },
        );
        let end = Expr::new(
            Span::synthetic(),
            ExprNode::Lit { value: Literal::Float { value: range.end } },
        );
        let numeric = is_a_check(&attr_ivar, "Numeric");
        let from_begin = send(attr_ivar.clone(), ">=", vec![begin]);
        let to_end = send(attr_ivar.clone(), if range.exclusive { "<" } else { "<=" }, vec![end]);
        bool_op(BoolOpKind::And, numeric, bool_op(BoolOpKind::And, from_begin, to_end))
    } else {
        let array_lit = Expr::new(
            Span::synthetic(),
            ExprNode::Array {
                elements: values
                    .iter()
                    .map(|lit| Expr::new(Span::synthetic(), ExprNode::Lit { value: lit.clone() }))
                    .collect(),
                style: ArrayStyle::Brackets,
            },
        );
        send(array_lit, "include?", vec![attr_ivar])
    };
    let push_err =
        errors_push(attr.as_str(), msgs.error(attr.as_str(), "exclusion", &Default::default()));
    if_with_nil_else(reserved, push_err)
}

/// Inline `validates :attr, acceptance: { accept: [...], allow_nil: }`.
/// Rails 8.1.4's `AcceptanceValidator` adds `:accepted` unless
/// `Array(accept).include?(value)`, and skips nil unless `allow_nil:
/// false`:
///   if !@attr.nil? && !(@attr == "1" || @attr == true)
///     errors << "attr must be accepted"
///   end
/// The test is one `==` per accepted value, which every target compiles
/// against any field type. A strict target's field is its column's type,
/// so a Boolean column compares only the Boolean values (Rails casts the
/// input before validating, and a cast `"1"` is `true`), and a String or
/// Integer column only those; with none left, nothing is accepted. The
/// ruby family compares them all, as Rails' `include?` does, except on a
/// Boolean column: its writer stores the input uncast, so the check reads
/// `ActiveSupport.cast_boolean(@attr)` (`"yes"` is `true`, `""` nil).
fn inline_acceptance_check(
    msgs: Messages,
    attr: &Symbol,
    accept: &[Literal],
    allow_nil: bool,
    attr_ty: Option<&Ty>,
    runtime_relation: super::RuntimeRelation,
) -> Expr {
    let (inner_ty, nilable) = peel_nilable(attr_ty);
    let boolean_column = matches!(inner_ty, Some(Ty::Bool));
    // The ruby family's column writer stores the input uncast, so a
    // Boolean column is cast here, as Rails casts it before validating.
    let cast = (runtime_relation == super::RuntimeRelation::Present && boolean_column).then(|| {
        let mut cast = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(Expr::new(Span::synthetic(), ExprNode::Const { path: vec![Symbol::from("ActiveSupport")] })),
                method: Symbol::from("cast_boolean"),
                args: vec![ivar(attr)],
                block: None,
                parenthesized: true,
            },
        );
        cast.ty = Some(Ty::Union { variants: vec![Ty::Bool, Ty::Nil].into() });
        cast
    });
    let value = match (&cast, runtime_relation, nilable, inner_ty) {
        (Some(cast), ..) => cast.clone(),
        (None, super::RuntimeRelation::Absent, true, Some(inner)) => narrowed_ivar(attr, inner),
        _ => ivar(attr),
    };
    let keeps = |lit: &Literal| match (runtime_relation, inner_ty) {
        (_, Some(Ty::Bool)) => matches!(lit, Literal::Bool { .. }),
        (super::RuntimeRelation::Present, _) | (_, None) => true,
        (_, Some(Ty::Str)) => matches!(lit, Literal::Str { .. }),
        (_, Some(Ty::Int)) => matches!(lit, Literal::Int { .. }),
        _ => false,
    };
    let accepted = accept
        .iter()
        .filter(|lit| keeps(lit))
        .map(|lit| send(value.clone(), "==", vec![Expr::new(Span::synthetic(), ExprNode::Lit { value: lit.clone() })]))
        .reduce(|acc, eq| bool_op(BoolOpKind::Or, acc, eq))
        .unwrap_or_else(|| Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value: false } }));
    let rejected = send(accepted, "!", vec![]);
    let nil = send(cast.unwrap_or_else(|| ivar(attr)), "nil?", vec![]);
    // A nil value is skipped under allow_nil and rejected otherwise; the
    // comparisons read the narrowed value behind the nil test.
    let invalid = if allow_nil {
        bool_op(BoolOpKind::And, send(nil, "!", vec![]), rejected)
    } else {
        bool_op(BoolOpKind::Or, nil, rejected)
    };
    let push_err = errors_push(attr.as_str(), msgs.error(attr.as_str(), "accepted", &Default::default()));
    if_with_nil_else(invalid, push_err)
}

/// Inline `validates :attr, format: { with: /pattern/ }`. Rails 8.1.4's
/// `FormatValidator` matches `value.to_s`, so nil is checked as `""` and
/// a non-String value by its String form (`true` passes `/\Atrue\z/`).
///   if !/pattern/.match?(@attr.to_s)
///     errors << "attr is invalid"
///   end
/// On the ruby family that is the whole check. A strict target's field
/// is its column's type: a String column matches the value itself, any
/// other matches its `to_s`, and a nullable column matches `""` for nil
/// (`(@attr.nil? && /pattern/.match?("")) || (!@attr.nil? &&
/// /pattern/.match?(<narrowed>))`), unless `allow_nil:` / `allow_blank:`
/// already skips nil.
/// The runtime helper's `with.nil?` guard is unnecessary — the
/// pattern is a known literal at lower time.
fn inline_format_check(
    msgs: Messages,
    attr: &Symbol,
    pattern: &str,
    flags: &str,
    attr_ty: Option<&Ty>,
    nil_skipped: bool,
    runtime_relation: super::RuntimeRelation,
) -> Expr {
    let matches = |value: Expr| {
        let regex_lit = Expr::new(
            Span::synthetic(),
            ExprNode::Lit {
                value: Literal::Regex { pattern: pattern.to_string(), flags: flags.to_string() },
            },
        );
        send(regex_lit, "match?", vec![value])
    };
    let valid = match runtime_relation {
        super::RuntimeRelation::Present => matches(send(ivar(attr), "to_s", vec![])),
        super::RuntimeRelation::Absent => {
            let (attr_ty, nilable) = peel_nilable(attr_ty);
            let value = match (nilable, attr_ty) {
                (true, Some(inner)) => narrowed_ivar(attr, inner),
                _ => ivar(attr),
            };
            let string_form = match attr_ty {
                Some(Ty::Str) => value,
                _ => send(value, "to_s", vec![]),
            };
            if nilable && !nil_skipped {
                // Two guarded disjuncts rather than a conditional
                // expression, which Go has no spelling for.
                let empty = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: String::new() } });
                let nil = send(ivar(attr), "nil?", vec![]);
                let present = send(nil.clone(), "!", vec![]);
                bool_op(
                    BoolOpKind::Or,
                    bool_op(BoolOpKind::And, nil, matches(empty)),
                    bool_op(BoolOpKind::And, present, matches(string_form)),
                )
            } else {
                matches(string_form)
            }
        }
    };
    let invalid = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(valid),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let push_err = errors_push(attr.as_str(), msgs.error(attr.as_str(), "invalid", &Default::default()));
    if_with_nil_else(invalid, push_err)
}

/// Inline `validates :attr, length: { minimum: M, maximum: N }`.
///
/// Rails' `LengthValidator` (8.1.4) does not treat nil as "skip" for
/// every bound. With `allow_nil` unset, a nil value fails `:minimum`
/// (the error is added without consulting the length) and is skipped
/// for a lone `:maximum` (`skip_nil_check?`). `allow_nil: true` skips
/// the whole check. `allow_nil: false` fails both bounds on nil.
///
/// The `is` (exact length) option isn't in the current ValidationRule
/// shape; left for when the IR adds it.
fn inline_length_check(
    msgs: Messages,
    attr: &Symbol,
    min: Option<usize>,
    max: Option<usize>,
    message: Option<&str>,
    attr_ty: Option<&Ty>,
    allow_nil: Option<bool>,
) -> Vec<Expr> {
    let (attr_ty, nilable) = peel_nilable(attr_ty);
    // Same narrowing as the presence check: the caller guards the
    // length computation with `!@attr.nil?`, so the read inside it is
    // the nil-excluded value.
    let attr_ivar = match (nilable, attr_ty) {
        (true, Some(inner)) => narrowed_ivar(attr, inner),
        _ => ivar(attr),
    };
    // Compute `len`. When the attr's column type is statically known
    // (Str / Array), drop the `is_a?` discrimination — `body : String`
    // can never be an Array, and tsc narrows the dead branch to
    // `never` and rejects the subsequent `.length`. Generic three-way
    // form retained for unknown/dynamic attrs.
    let length_send = send(attr_ivar.clone(), "length", vec![]);
    let len_expr = match attr_ty {
        Some(Ty::Str) | Some(Ty::Array { .. }) => length_send,
        _ => {
            let zero_lit = Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: 0 } },
            );
            let inner_else = Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: is_a_check(&attr_ivar, "Array"),
                    then_branch: length_send.clone(),
                    else_branch: zero_lit,
                },
            );
            Expr::new(
                Span::synthetic(),
                ExprNode::If {
                    cond: is_a_check(&attr_ivar, "String"),
                    then_branch: length_send,
                    else_branch: inner_else,
                },
            )
        }
    };
    let len_var = Symbol::from("len");
    let len_assign = Expr::new(
        Span::synthetic(),
        ExprNode::Assign {
            target: crate::expr::LValue::Var {
                id: crate::ident::VarId(0),
                name: len_var.clone(),
            },
            value: len_expr,
        },
    );
    let len_read = Expr::new(
        Span::synthetic(),
        ExprNode::Var { id: crate::ident::VarId(0), name: len_var },
    );

    let mut inner_stmts: Vec<Expr> = vec![len_assign];
    let mut nil_errors: Vec<Expr> = Vec::new();
    // One pass over the two bounds. `nil_fails_bound` is Rails'
    // `skip_nil_check?` plus `allow_nil`: unset fails minimum only,
    // `true` skips nil, `false` fails both.
    for (bound, n, kind, op) in [
        (LengthBound::Min, min, "too_short", "<"),
        (LengthBound::Max, max, "too_long", ">"),
    ] {
        let Some(n) = n else { continue };
        let cmp = send(
            len_read.clone(),
            op,
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Int { value: n as i64 } },
            )],
        );
        let push = length_error_push(&msgs, attr, kind, n, message);
        inner_stmts.push(if_with_nil_else(cmp, push));
        if nil_fails_bound(bound, allow_nil) {
            nil_errors.push(length_error_push(&msgs, attr, kind, n, message));
        }
    }
    let body_seq = seq(inner_stmts);

    // The guard tests the RAW ivar. `attr_ivar` above may be the
    // narrowed `Cast` read, which is what the non-nil arm exists to
    // make safe.
    let nil_check = send(ivar(attr), "nil?", vec![]);
    let not_nil = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(nil_check.clone()),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    if nil_errors.is_empty() {
        // `allow_nil: true`, or a lone maximum (Rails skips nil).
        return vec![if_with_nil_else(not_nil, body_seq)];
    }
    let nil_arm = if nil_errors.len() == 1 {
        nil_errors.pop().unwrap()
    } else {
        seq(nil_errors)
    };
    vec![Expr::new(
        Span::synthetic(),
        ExprNode::If { cond: nil_check, then_branch: nil_arm, else_branch: body_seq },
    )]
}

enum LengthBound {
    Min,
    Max,
}

/// Whether a nil value fails this bound. `None` is Rails' default
/// (`skip_nil_check?` is true only for `:maximum`), not a missing flag.
fn nil_fails_bound(bound: LengthBound, allow_nil: Option<bool>) -> bool {
    match allow_nil {
        Some(true) => false,
        Some(false) => true,
        None => matches!(bound, LengthBound::Min),
    }
}

fn length_error_push(msgs: &Messages, attr: &Symbol, kind: &str, n: usize, message: Option<&str>) -> Expr {
    let msg = match message {
        Some(m) => msgs.error(
            attr.as_str(),
            kind,
            &ErrorOpts { message: Some(m.to_string()), ..ErrorOpts::count(n) },
        ),
        None => msgs.error(attr.as_str(), kind, &ErrorOpts::count(n)),
    };
    errors_push(attr.as_str(), msg)
}

/// Inline `validates :attr, numericality: { ... }`.
///   if @attr.nil? || !@attr.is_a?(Numeric)
///     errors << "attr is not a number"
///   else
///     errors << "attr must be greater than G" if @attr <= G      # if gt set
///     errors << "attr must be less than L"    if @attr >= L      # if lt set
///     errors << "attr must be greater than or equal to G" if @attr < G  # if gte set
///     errors << "attr must be less than or equal to L"    if @attr > L  # if lte set
///     errors << "attr must be an integer"     if !@attr.is_a?(Integer)  # if only_integer
///   end
/// The if/else form keeps subsequent rules on other attrs running:
/// no early `return` from within `def validate`.
fn inline_numericality_check(
    msgs: Messages,
    attr: &Symbol,
    only_integer: bool,
    gt: Option<f64>,
    lt: Option<f64>,
    gte: Option<f64>,
    lte: Option<f64>,
    in_range: Option<&crate::dialect::InclusionRange>,
) -> Vec<Expr> {
    let attr_ivar = ivar(attr);
    // `@attr.nil? || !@attr.is_a?(Numeric)` — the "not a number" guard.
    let nil_check = send(attr_ivar.clone(), "nil?", vec![]);
    let is_numeric = is_a_check(&attr_ivar, "Numeric");
    let not_numeric = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(is_numeric),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let bad_cond = bool_op(BoolOpKind::Or, nil_check, not_numeric);
    let nan_msg = errors_push(attr.as_str(), msgs.error(attr.as_str(), "not_a_number", &Default::default()));

    // Build the else-branch Seq of per-option checks.
    let mut else_stmts: Vec<Expr> = Vec::new();
    if let Some(n) = gt {
        let le = send(
            attr_ivar.clone(),
            "<=",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Float { value: n } },
            )],
        );
        let msg = msgs.error(attr.as_str(), "greater_than", &ErrorOpts::count(format_float(n)));
        else_stmts.push(if_with_nil_else(le, errors_push(attr.as_str(), msg)));
    }
    if let Some(n) = lt {
        let ge = send(
            attr_ivar.clone(),
            ">=",
            vec![Expr::new(
                Span::synthetic(),
                ExprNode::Lit { value: Literal::Float { value: n } },
            )],
        );
        let msg = msgs.error(attr.as_str(), "less_than", &ErrorOpts::count(format_float(n)));
        else_stmts.push(if_with_nil_else(ge, errors_push(attr.as_str(), msg)));
    }
    if let Some(n) = gte {
        let below = send(
            attr_ivar.clone(),
            "<",
            vec![Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value: n } })],
        );
        let msg = msgs.error(
            attr.as_str(),
            "greater_than_or_equal_to",
            &ErrorOpts::count(format_float(n)),
        );
        else_stmts.push(if_with_nil_else(below, errors_push(attr.as_str(), msg)));
    }
    if let Some(n) = lte {
        let above = send(
            attr_ivar.clone(),
            ">",
            vec![Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value: n } })],
        );
        let msg = msgs.error(
            attr.as_str(),
            "less_than_or_equal_to",
            &ErrorOpts::count(format_float(n)),
        );
        else_stmts.push(if_with_nil_else(above, errors_push(attr.as_str(), msg)));
    }
    // `in: a..b`: `@attr < a || @attr > b` (`>= b` for `a...b`), and
    // Rails' message names the range as `Range#inspect` prints it.
    if let Some(range) = in_range {
        let float = |n: f64| Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Float { value: n } });
        let below = send(attr_ivar.clone(), "<", vec![float(range.begin)]);
        let above = send(attr_ivar.clone(), if range.exclusive { ">=" } else { ">" }, vec![float(range.end)]);
        let dots = if range.exclusive { "..." } else { ".." };
        let msg = msgs.error(
            attr.as_str(),
            "in",
            &ErrorOpts::count(format!("{}{dots}{}", format_float(range.begin), format_float(range.end))),
        );
        else_stmts.push(if_with_nil_else(bool_op(BoolOpKind::Or, below, above), errors_push(attr.as_str(), msg)));
    }
    if only_integer {
        let is_int = is_a_check(&attr_ivar, "Integer");
        let not_int = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: Some(is_int),
                method: Symbol::from("!"),
                args: vec![],
                block: None,
                parenthesized: false,
            },
        );
        let msg = msgs.error(attr.as_str(), "not_an_integer", &Default::default());
        else_stmts.push(if_with_nil_else(not_int, errors_push(attr.as_str(), msg)));
    }
    // If the else has no stmts, just use the if-form (the rule has
    // only the implicit "is a number" check). Otherwise wrap in
    // if/else with both branches populated.
    if else_stmts.is_empty() {
        return vec![if_with_nil_else(bad_cond, nan_msg)];
    }
    let else_branch = seq(else_stmts);
    vec![Expr::new(
        Span::synthetic(),
        ExprNode::If { cond: bad_cond, then_branch: nan_msg, else_branch },
    )]
}

/// Float literal formatter that matches Ruby's default `to_s` shape
/// for whole numbers: `5.0` → "5", `0.5` → "0.5". Matches the runtime
/// helper's `#{greater_than}` interpolation output so error messages
/// agree across inline and helper-call paths.
fn format_float(n: f64) -> String {
    if n == n.trunc() {
        // Through `i128`: `2**63` is a whole f64 that `as i64` would
        // saturate to `i64::MAX`.
        format!("{}", n as i128)
    } else {
        format!("{n}")
    }
}

/// What a baked message needs from its model: the attribute's
/// `human_attribute_name` and the locale's `errors.format`.
#[derive(Clone, Copy, Default)]
/// The second field is a validation's own `message:` String, used in
/// place of each error's default (see [`Messages::error`]).
pub(crate) struct Messages<'a>(pub(crate) Option<&'a Model>, pub(crate) Option<&'a str>);

use crate::i18n::ErrorOpts;

impl Messages<'_> {
    /// The full message Rails stores for an error of `kind` on `attr`, with the error it records.
    fn error(&self, attr: &str, kind: &str, opts: &ErrorOpts) -> Message {
        // A rule-level message (Length's) already in `opts` wins; else the
        // validation's own `message:`, which the lookup uses after a
        // model-specific translation, as Rails' `generate_message` does.
        let overridden;
        let opts = match (self.1, &opts.message) {
            (Some(message), None) => {
                overridden = ErrorOpts { message: Some(message.to_string()), ..opts.clone() };
                &overridden
            }
            _ => opts,
        };
        let default = crate::i18n::ModelI18n::default();
        let i18n = self.0.map(|m| &m.i18n).unwrap_or(&default);
        let parts = |p| value_parts_expr(p, attr);
        Message {
            full: parts(i18n.full_error(attr, kind, opts)),
            kind: kind.to_string(),
            bare: parts(i18n.error(attr, kind, opts)),
            // The others carry `count:`, `value:` or `attribute:` beside the type in Rails' `details`.
            optionless: matches!(kind, "blank" | "present" | "required" | "accepted" | "password_too_long"),
        }
    }
}

/// A message's parts as one String expression, `%{value}` read as
/// `attr.to_s`: what I18n renders (nil as ""), and a String on every
/// target, where a nullable field's bare read is an `Option` a strict
/// target can't interpolate.
fn value_parts_expr(parts: Vec<crate::i18n::Part>, attr: &str) -> Expr {
    if !parts.iter().any(|p| matches!(p, crate::i18n::Part::Value)) {
        return crate::lower::errors_add::parts_expr(Span::synthetic(), parts, None, attr);
    }
    let pieces = parts
        .into_iter()
        .map(|p| match p {
            crate::i18n::Part::Text(value) => crate::expr::InterpPart::Text { value },
            crate::i18n::Part::Value => crate::expr::InterpPart::Expr {
                expr: send(
                    Expr::new(
                        Span::synthetic(),
                        ExprNode::Send { recv: None, method: Symbol::from(attr), args: vec![], block: None, parenthesized: false },
                    ),
                    "to_s",
                    vec![],
                ),
            },
        })
        .collect();
    Expr::new(Span::synthetic(), ExprNode::StringInterp { parts: pieces })
}

pub(crate) struct Message {
    full: Expr,
    kind: String,
    bare: Expr,
    optionless: bool,
}

// ── IR helpers ────────────────────────────────────────────────

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

#[derive(Clone, Copy)]
enum AttachmentKind {
    One,
    Many,
}

/// Whether `attr` is a `has_one_attached` / `has_many_attached` the
/// model declares.
fn attachment_kind(model: &Model, attr: &Symbol) -> Option<AttachmentKind> {
    if crate::lower::attached::attached_attrs(model).iter().any(|(_, a)| a == attr) {
        Some(AttachmentKind::One)
    } else if crate::lower::attached::many_attached_attrs(model).iter().any(|(_, a)| a == attr) {
        Some(AttachmentKind::Many)
    } else {
        None
    }
}

/// The attachment is blank — Rails' `Attached::One#blank?` (`!attached?`)
/// and `Attached::Many`'s `attachments.empty?`, which both read the
/// change staged by the writer before the saved rows:
///
/// ```ruby
/// @image_pending.nil? && !image.attached?   # has_one_attached
/// !images.attached?                         # has_many_attached
/// ```
///
/// The staged half is what makes `Photo.new(image: file).valid?` true:
/// validation runs before the save that writes the attachment row. Only
/// `has_one_attached` has a synthesized writer (`lower::attached`), and
/// only when the model does not define the reader itself; a
/// `has_many_attached` attaches through the proxy, which writes its rows
/// at once.
fn attachment_blank_cond(model: &Model, attr: &Symbol, kind: AttachmentKind) -> Expr {
    let reader = Expr::new(
        Span::synthetic(),
        ExprNode::Send { recv: None, method: attr.clone(), args: vec![], block: None, parenthesized: false },
    );
    let not_attached = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: Some(send(reader, "attached?", vec![])),
            method: Symbol::from("!"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let staged_writer = matches!(kind, AttachmentKind::One)
        && !super::model_defines_instance_method(model, attr);
    if !staged_writer {
        return not_attached;
    }
    let pending = Expr::new(
        Span::synthetic(),
        ExprNode::Ivar { name: Symbol::from(format!("{}_pending", attr.as_str())) },
    );
    bool_op(BoolOpKind::And, send(pending, "nil?", vec![]), not_attached)
}

fn bool_op(op: BoolOpKind, left: Expr, right: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::BoolOp { op, surface: BoolOpSurface::default(), left, right },
    )
}

fn is_a_check(value: &Expr, class_name: &str) -> Expr {
    let class_const = Expr::new(
        Span::synthetic(),
        ExprNode::Const { path: vec![Symbol::from(class_name)] },
    );
    send(value.clone(), "is_a?", vec![class_const])
}

/// `errors << <msg_expr>` and `error_details << ActiveRecord::ErrorDetail.new(...)` —
/// pushes a String literal onto the `errors` collection, and the
/// error it records onto its index-for-index companion. `errors` is reached via implicit-self Send (the same
/// shape every existing validates_*_of helper produces inside the
/// Validations module).
fn errors_push(attribute: &str, msg: Message) -> Expr {
    let errors_call = Expr::new(
        Span::synthetic(),
        ExprNode::Send {
            recv: None,
            method: Symbol::from("errors"),
            args: vec![],
            block: None,
            parenthesized: false,
        },
    );
    let kind = Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Str { value: msg.kind } });
    crate::lower::errors_add::attributed_push(send(errors_call, "<<", vec![msg.full]), None, attribute, kind, true, msg.optionless, msg.bare, Span::synthetic())
}

fn if_with_nil_else(cond: Expr, then_branch: Expr) -> Expr {
    let nil_lit = Expr::new(
        Span::synthetic(),
        ExprNode::Lit { value: Literal::Nil },
    );
    Expr::new(
        Span::synthetic(),
        ExprNode::If { cond, then_branch, else_branch: nil_lit },
    )
}

#[cfg(test)]
mod tests {
    //! Unit tests per ValidationRule arm. Replaces the deleted
    //! `runtime/ruby/test/active_record/validations_test.rb` framework
    //! test, which tested the runtime `validates_*_of` helper methods —
    //! a surface no Group 1 target dispatches into any more (Phase 2.5(a)
    //! inlines every validates declaration here).
    //!
    //! Each test calls the inline helper directly and asserts on the
    //! error-message strings the lowered IR emits, since the error text
    //! is the user-visible contract. Structural shape is covered
    //! implicitly: the message-collection walker only reaches strings
    //! that sit at `errors << "..."` positions.
    use super::*;
    use crate::ident::ClassId;

    /// Walk an `Expr` tree and collect every string literal that's the
    /// RHS of an `errors << "..."` send (the canonical error-push shape
    /// emitted by `errors_push`). Used to assert which error messages
    /// the lowerer arms produce.
    fn collect_error_messages(expr: &Expr) -> Vec<String> {
        let mut out = Vec::new();
        walk(expr, &mut out);
        out
    }

    fn walk(expr: &Expr, out: &mut Vec<String>) {
        match expr.node.as_ref() {
            ExprNode::Send { recv, method, args, .. } => {
                if method.as_str() == "<<" {
                    if let Some(r) = recv.as_ref() {
                        if let ExprNode::Send { method: m, recv: None, .. } = r.node.as_ref() {
                            if m.as_str() == "errors" {
                                if let Some(arg) = args.first() {
                                    if let ExprNode::Lit { value: Literal::Str { value } } =
                                        arg.node.as_ref()
                                    {
                                        out.push(value.clone());
                                    }
                                }
                            }
                        }
                    }
                }
                if let Some(r) = recv {
                    walk(r, out);
                }
                for a in args {
                    walk(a, out);
                }
            }
            ExprNode::BoolOp { left, right, .. } => {
                walk(left, out);
                walk(right, out);
            }
            ExprNode::If { cond, then_branch, else_branch } => {
                walk(cond, out);
                walk(then_branch, out);
                walk(else_branch, out);
            }
            ExprNode::Seq { exprs } => {
                for e in exprs {
                    walk(e, out);
                }
            }
            ExprNode::Assign { value, .. } => walk(value, out),
            _ => {}
        }
    }

    fn attr() -> Symbol {
        Symbol::from("title")
    }

    #[test]
    fn presence_emits_blank_error() {
        let expr = inline_presence_check(Messages::default(), &attr(), None, super::super::RuntimeRelation::Absent);
        assert_eq!(collect_error_messages(&expr), vec!["Title can't be blank"]);
    }

    #[test]
    fn presence_str_typed_attr_drops_is_a_array_branch() {
        // Statically-typed string attr: the `is_a?(Array) && ...` arm
        // must drop out, otherwise tsc narrows the dead branch to
        // `never` and rejects the subsequent `.length` access.
        let expr = inline_presence_check(Messages::default(), &attr(), Some(&Ty::Str), super::super::RuntimeRelation::Absent);
        let dbg = format!("{:?}", expr);
        assert!(
            !dbg.contains("\"Array\""),
            "Str-typed attr should drop is_a?(Array); tree: {dbg}",
        );
    }

    #[test]
    fn presence_array_typed_attr_drops_is_a_string_branch() {
        let expr = inline_presence_check(Messages::default(), &attr(), Some(&Ty::Array { elem: std::sync::Arc::new(Ty::Str) }), super::super::RuntimeRelation::Absent);
        let dbg = format!("{:?}", expr);
        assert!(
            !dbg.contains("\"String\""),
            "Array-typed attr should drop is_a?(String); tree: {dbg}",
        );
    }

    #[test]
    fn absence_emits_must_be_blank_error() {
        let expr = inline_absence_check(Messages::default(), &attr(), None, super::super::RuntimeRelation::Absent);
        assert_eq!(collect_error_messages(&expr), vec!["Title must be blank"]);
    }

    #[test]
    fn length_min_only_emits_too_short() {
        let exprs = inline_length_check(Messages::default(), &attr(), Some(5), None, None, None, None);
        assert_eq!(exprs.len(), 1, "length lowers to one outer expression");
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        // Nil fails the minimum outright, and a present value fails when shorter.
        assert_eq!(
            msgs,
            vec![
                "Title is too short (minimum is 5 characters)",
                "Title is too short (minimum is 5 characters)",
            ],
        );
        let (cond, then_branch, else_branch) = root_if(&exprs);
        assert!(is_nil_predicate(cond), "nil takes the too-short arm");
        assert_eq!(collect_error_messages(then_branch), vec!["Title is too short (minimum is 5 characters)"]);
        assert!(!is_nil_lit(else_branch), "the present value still measures length");
    }

    #[test]
    fn length_max_only_skips_nil() {
        let exprs = inline_length_check(Messages::default(), &attr(), None, Some(100), None, None, None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert_eq!(msgs, vec!["Title is too long (maximum is 100 characters)"]);
        let (cond, _, else_branch) = root_if(&exprs);
        assert!(is_not_nil(cond), "a lone maximum skips nil, as Rails does");
        assert!(is_nil_lit(else_branch));
    }

    #[test]
    fn length_allow_nil_skips_the_minimum() {
        let exprs = inline_length_check(
            Messages::default(),
            &attr(),
            Some(5),
            None,
            None,
            None,
            Some(true),
        );
        let (cond, _, else_branch) = root_if(&exprs);
        assert!(is_not_nil(cond), "allow_nil skips the check when the value is nil");
        assert!(is_nil_lit(else_branch));
    }

    #[test]
    fn length_allow_nil_false_fails_a_maximum_on_nil() {
        let exprs = inline_length_check(
            Messages::default(),
            &attr(),
            None,
            Some(100),
            None,
            None,
            Some(false),
        );
        let (cond, then_branch, _) = root_if(&exprs);
        assert!(is_nil_predicate(cond));
        assert_eq!(
            collect_error_messages(then_branch),
            vec!["Title is too long (maximum is 100 characters)"],
        );
    }

    #[test]
    fn length_min_and_max_emits_both_in_order() {
        let exprs = inline_length_check(Messages::default(), &attr(), Some(5), Some(100), None, None, None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert_eq!(
            msgs,
            vec![
                "Title is too short (minimum is 5 characters)",
                "Title is too short (minimum is 5 characters)",
                "Title is too long (maximum is 100 characters)",
            ],
        );
    }

    fn root_if(exprs: &[Expr]) -> (&Expr, &Expr, &Expr) {
        let ExprNode::If { cond, then_branch, else_branch } = exprs[0].node.as_ref() else {
            panic!("length check is an if");
        };
        (cond, then_branch, else_branch)
    }

    fn is_nil_predicate(expr: &Expr) -> bool {
        matches!(expr.node.as_ref(), ExprNode::Send { method, .. } if method.as_str() == "nil?")
    }

    fn is_not_nil(expr: &Expr) -> bool {
        matches!(
            expr.node.as_ref(),
            ExprNode::Send { method, recv: Some(recv), .. }
                if method.as_str() == "!" && is_nil_predicate(recv)
        )
    }

    fn is_nil_lit(expr: &Expr) -> bool {
        matches!(expr.node.as_ref(), ExprNode::Lit { value: Literal::Nil })
    }

    #[test]
    fn format_emits_invalid_error() {
        let expr = inline_format_check(Messages::default(), &attr(), "[A-Z]+", "", None, false, crate::lower::model_to_library::RuntimeRelation::Absent);
        assert_eq!(collect_error_messages(&expr), vec!["Title is invalid"]);
    }

    /// The regex literal `inline_format_check` builds carries the
    /// flags it's given — before this, `Literal::Regex { flags: .. }`
    /// was always hardcoded empty, so a model's `i`/`m` flag never
    /// reached any emit target even once ingest stopped refusing it.
    #[test]
    fn format_check_carries_flags_into_the_regex_literal() {
        let expr = inline_format_check(Messages::default(), &attr(), "a.b", "im", None, false, crate::lower::model_to_library::RuntimeRelation::Absent);
        let flags = find_regex_flags(&expr).expect("expected a Regex literal in the format check");
        assert_eq!(flags, "im");
    }

    fn find_regex_flags(expr: &Expr) -> Option<String> {
        match expr.node.as_ref() {
            ExprNode::Lit {
                value: Literal::Regex { flags, .. },
            } => Some(flags.clone()),
            ExprNode::Send { recv, args, .. } => recv
                .as_ref()
                .and_then(|r| find_regex_flags(r))
                .or_else(|| args.iter().find_map(find_regex_flags)),
            ExprNode::If { cond, then_branch, else_branch } => find_regex_flags(cond)
                .or_else(|| find_regex_flags(then_branch))
                .or_else(|| find_regex_flags(else_branch)),
            ExprNode::BoolOp { left, right, .. } => {
                find_regex_flags(left).or_else(|| find_regex_flags(right))
            }
            ExprNode::Seq { exprs } => exprs.iter().find_map(find_regex_flags),
            _ => None,
        }
    }

    #[test]
    fn inclusion_emits_not_included_error() {
        let values = vec![
            Literal::Str { value: "a".into() },
            Literal::Str { value: "b".into() },
        ];
        let expr = inline_inclusion_check(Messages::default(), &attr(), &values, None);
        assert_eq!(
            collect_error_messages(&expr),
            vec!["Title is not included in the list"],
        );
    }

    #[test]
    fn numericality_bare_emits_nan_only() {
        let exprs = inline_numericality_check(Messages::default(), &attr(), false, None, None, None, None, None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert_eq!(msgs, vec!["Title is not a number"]);
    }

    #[test]
    fn numericality_with_gt_lt_and_only_integer_emits_all_messages() {
        let exprs = inline_numericality_check(Messages::default(), &attr(), true, Some(0.0), Some(100.0), None, None, None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        // Order matches the source order in inline_numericality_check:
        // nan-msg (then-branch), then gt → lt → only_integer in else.
        assert_eq!(
            msgs,
            vec![
                "Title is not a number",
                "Title must be greater than 0",
                "Title must be less than 100",
                "Title must be an integer",
            ],
        );
    }

    #[test]
    fn numericality_float_bounds_format_without_trailing_zero() {
        // `format_float` matches Ruby's default `to_s` shape for whole
        // numbers: 5.0 → "5", 0.5 → "0.5". Lock the contract so error
        // messages stay byte-stable across cruby and transpiled targets.
        let exprs = inline_numericality_check(Messages::default(), &attr(), false, Some(0.5), None, None, None, None);
        let msgs: Vec<String> = exprs.iter().flat_map(collect_error_messages).collect();
        assert!(
            msgs.iter().any(|m| m == "Title must be greater than 0.5"),
            "expected decimal preserved; got {msgs:?}",
        );
    }

    #[test]
    fn belongs_to_inline_emits_must_exist_error() {
        let assoc_name = Symbol::from("article");
        let foreign_key = Symbol::from("article_id");
        let expr = inline_belongs_to_check(Messages::default(), &assoc_name, &foreign_key, false);
        assert_eq!(collect_error_messages(&expr), vec!["Article must exist"]);
    }

    #[test]
    fn belongs_to_checks_the_target_through_its_reader_not_a_query() {
        // Rails' presence validation reads the association, so a loaded
        // or assigned target answers without asking the database. The
        // reader goes by the association's `primary_key:` itself.
        let assoc_name = Symbol::from("product");
        let foreign_key = Symbol::from("product_uuid");
        let expr = inline_belongs_to_check(Messages::default(), &assoc_name, &foreign_key, false);
        let rendered = format!("{expr:?}");
        assert!(!rendered.contains("exists?"), "the check must not probe with exists?: {rendered}");
        assert!(rendered.contains("method: Symbol(\"product\")"), "the check reads the association: {rendered}");
        assert_eq!(collect_error_messages(&expr), vec!["Product must exist"]);
    }

    #[test]
    fn polymorphic_belongs_to_checks_the_id_but_names_no_class() {
        // The class to query lives in the row's `<assoc>_type` column,
        // and `target` is the assoc-name phantom — writing it out was an
        // uninitialized-constant NameError on the first save.
        let assoc_name = Symbol::from("record");
        let foreign_key = Symbol::from("record_id");
        let expr = inline_belongs_to_check(Messages::default(), &assoc_name, &foreign_key, true);
        assert_eq!(collect_error_messages(&expr), vec!["Record must exist"]);
        let rendered = format!("{expr:?}");
        assert!(
            !rendered.contains("exists?"),
            "a polymorphic check must not query a class: {rendered}"
        );
    }
}

/// `Union{[T, Nil]}` → `T`, for the nullable-column slot types the
/// attributes row now carries. The checks below discriminate on the
/// underlying type and narrow the read with `Cast` — the IR's
/// nilable→scalar bridge — under a `nil?` guard that short-circuits in
/// every target.
fn peel_nilable(ty: Option<&Ty>) -> (Option<&Ty>, bool) {
    match ty {
        Some(Ty::Union { variants }) if variants.len() == 2 => {
            let inner = variants.iter().find(|v| !matches!(v, Ty::Nil));
            let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
            match (inner, has_nil) {
                (Some(i), true) => (Some(i), true),
                _ => (ty, false),
            }
        }
        other => (other, false),
    }
}

/// `Cast(@attr, T)` — the nil-excluded read. Callers place it only
/// where a preceding `nil?` disjunct/conjunct guarantees the value.
/// `value.strip`, typed as the String it is. The strict String arms ask
/// `empty?` of it; an emitter that dispatches `empty?` by the receiver's
/// type (elixir: `s == ""`) needs that type on the synthesized call,
/// which has none of its own, or it falls back to a struct dispatch.
fn stripped_str(value: Expr) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Cast { value: send(value, "strip", vec![]), target_ty: Ty::Str },
    )
}

fn narrowed_ivar(attr: &Symbol, inner: &Ty) -> Expr {
    Expr::new(
        Span::synthetic(),
        ExprNode::Cast { value: ivar(attr), target_ty: inner.clone() },
    )
}
