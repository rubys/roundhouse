//! `has_secure_password` — synthesize the methods Rails' macro
//! provides, in the shared model lowering (all targets): the
//! authenticator (`authenticate`, or `authenticate_<attr>` for a
//! custom attribute) returning the record or `false`, plus the
//! plaintext virtual-attribute accessors (`password` / `password=` /
//! `password_confirmation` / `password_confirmation=`).
//!
//! The bodies call the bcrypt gem's own surface (`BCrypt::Password.
//! create/new`) VERBATIM — deliberately not a roundhouse intrinsic:
//! per the spin mirror-naming policy (spinel#1753), a future
//! `spinel-bcrypt` native-`[[build]]` package claims `require
//! "bcrypt"` and satisfies this exact contract, so the emitted code
//! runs unchanged on the CRuby/JRuby trees (which load the real gem
//! via the overlay's guarded require) and on spinel once the package
//! lands. Until then strict targets and plain spinel carry the calls
//! as ONE named runtime seam — the bucket-3 posture from the gem-fate
//! taxonomy, and the "has_secure_password expected" entry on the AOT
//! probe's peel list.
//!
//! The analyzer already types this surface
//! (`register_has_secure_password`); signatures here mirror it —
//! writers take the plaintext `Str`, and the authenticator returns
//! the model instance (the dominant truthy use; the runtime `false`
//! arm is the analyzer's deliberate simplification, kept in
//! agreement here).
//!
//! Shared home of what used to be the ruby-family emit pass
//! `apply_secure_password_lowering`. Strict no-op for marker-free
//! apps (the blog).

use super::model_to_library::{fn_sig, push_synth_instance_method};
use crate::app::App;
use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Model, ModelBodyItem, Param};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{Symbol, VarId};
use crate::span::Span;
use crate::ty::Ty;

/// Synthesize the `has_secure_password` method family onto `model`'s
/// lowered methods. Custom methods in the model body win, as do names
/// an earlier synthesizer claimed.
pub(crate) fn push_secure_password_methods(methods: &mut Vec<MethodDef>, model: &Model) {
    let Some(attr) = secure_password_attr(&model.body) else {
        return;
    };
    let digest = Symbol::from(format!("{}_digest", attr.as_str()));
    let confirmation = Symbol::from(format!("{}_confirmation", attr.as_str()));
    // Rails names the authenticator after the attribute, except the
    // default `password` which gets the bare `authenticate`.
    let auth_name = authenticator_name(&attr);
    let plain = Symbol::from("unencrypted_password");
    let self_ty = Ty::Class { id: model.name.clone(), args: vec![] };
    let plaintext_ty = Ty::Union { variants: vec![Ty::Str, Ty::Nil] };
    push_synth_instance_method(
        methods,
        model,
        auth_name,
        vec![Param::positional(plain.clone())],
        authenticate_body(&digest),
        Some(fn_sig(vec![(plain.clone(), Ty::Str)], self_ty)),
        AccessorKind::Method,
        false,
    );
    // Plaintext virtual attribute: reader is a plain ivar read (nil
    // until a writer runs in this process — the digest column is the
    // persistent side), writer stores the plaintext AND the bcrypt
    // digest.
    push_synth_instance_method(
        methods,
        model,
        attr.clone(),
        Vec::new(),
        ivar_read(&attr),
        Some(fn_sig(vec![], plaintext_ty.clone())),
        AccessorKind::AttributeReader,
        false,
    );
    push_synth_instance_method(
        methods,
        model,
        Symbol::from(format!("{}=", attr.as_str())),
        vec![Param::positional(plain.clone())],
        plaintext_writer_body(&attr, &digest),
        Some(fn_sig(vec![(plain, Ty::Str)], Ty::Nil)),
        AccessorKind::Method,
        true,
    );
    push_synth_instance_method(
        methods,
        model,
        confirmation.clone(),
        Vec::new(),
        ivar_read(&confirmation),
        Some(fn_sig(vec![], plaintext_ty)),
        AccessorKind::AttributeReader,
        false,
    );
    let value = Symbol::from("value");
    push_synth_instance_method(
        methods,
        model,
        Symbol::from(format!("{}=", confirmation.as_str())),
        vec![Param::positional(value.clone())],
        plain_ivar_assign(&confirmation, &value),
        Some(fn_sig(vec![(value, Ty::Str)], Ty::Nil)),
        AccessorKind::AttributeWriter,
        true,
    );
    push_reset_token_methods(methods, model, &attr);
}

/// Rails 8's `has_secure_password` also generates a password-reset
/// token (`reset_token: true` is its default): `<attr>_reset_token`
/// mints it and the class-side `find_by_<attr>_reset_token(!)` read it
/// back, through `generates_token_for :"<attr>_reset", expires_in:
/// 15.minutes { <attr>_salt&.last(10) }`. The authentication generator's
/// PasswordsController and mailer are built on the three.
///
/// The model name, purpose and expiry are compile-time facts, folded
/// into the call so the runtime (`runtime/ruby/active_record/
/// token_for.rb`) needs no reflection. A `reset_token:` option other
/// than `true` is not reproduced: `false` means Rails defines none of
/// these, and a custom expiry would need its Duration read here.
fn push_reset_token_methods(methods: &mut Vec<MethodDef>, model: &Model, attr: &Symbol) {
    if !default_reset_token(&model.body) {
        return;
    }
    let digest = Symbol::from(format!("{}_digest", attr.as_str()));
    let purpose = format!("{}\\n{}_reset\\n{}", model.name.0.as_str(), attr.as_str(), RESET_TOKEN_TTL);
    let self_ty = Ty::Class { id: model.name.clone(), args: vec![] };
    let token_for = || sp_expr(ExprNode::Const {
        path: vec![Symbol::from("ActiveRecord"), Symbol::from("TokenFor")],
    });
    let purpose_lit = || sp_expr(ExprNode::Lit { value: Literal::Str { value: purpose.clone() } });
    let var = |name: &str| sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from(name) });
    // `[id, digest salt tail]` as JSON, for the record `id`/`digest` name.
    let data_of = |id: Expr, digest: Expr| send(Some(token_for()), "secure_password_data", vec![id, digest]);

    // def password_reset_token
    //   ActiveRecord::TokenFor.generate(ActiveRecord::TokenFor
    //     .secure_password_data(id, @password_digest), "User\npassword_reset\n900", 900)
    let generate = send(
        Some(token_for()),
        "generate",
        vec![
            data_of(send(Some(sp_expr(ExprNode::SelfRef)), "id", vec![]), ivar_read(&digest)),
            purpose_lit(),
            sp_expr(ExprNode::Lit { value: Literal::Int { value: RESET_TOKEN_TTL } }),
        ],
    );
    push_synth_instance_method(
        methods,
        model,
        Symbol::from(format!("{}_reset_token", attr.as_str())),
        Vec::new(),
        generate,
        Some(fn_sig(vec![], Ty::Str)),
        AccessorKind::Method,
        false,
    );

    // def password_reset_token_expires_in = 900 — Rails answers the
    // Duration `15.minutes`; seconds are what every reader here takes
    // (the reset mailer's `distance_of_time_in_words(0, …)`).
    push_synth_instance_method(
        methods,
        model,
        Symbol::from(format!("{}_reset_token_expires_in", attr.as_str())),
        Vec::new(),
        sp_expr(ExprNode::Lit { value: Literal::Int { value: RESET_TOKEN_TTL } }),
        Some(fn_sig(vec![], Ty::Int)),
        AccessorKind::Method,
        false,
    );

    // Both finders open the same way:
    //   data = ActiveRecord::TokenFor.verified_data(token, PURPOSE)
    let token = Symbol::from("token");
    let assign = |name: &str, value: Expr| sp_expr(ExprNode::Assign {
        target: LValue::Var { id: VarId(0), name: Symbol::from(name) },
        value,
    });
    let verify = || assign("data", send(Some(token_for()), "verified_data", vec![var("token"), purpose_lit()]));
    let id_of_data = || send(Some(token_for()), "data_id", vec![var("data")]);
    // The record still carries the salt the token was minted under.
    let still_matches = || send(
        Some(data_of(
            send(Some(var("record")), "id", vec![]),
            send(Some(var("record")), digest.as_str(), vec![]),
        )),
        "==",
        vec![var("data")],
    );
    let nil = || sp_expr(ExprNode::Lit { value: Literal::Nil });
    let invalid = || sp_expr(ExprNode::Raise {
        value: sp_expr(ExprNode::Const {
            path: vec![
                Symbol::from("ActiveSupport"),
                Symbol::from("MessageVerifier"),
                Symbol::from("InvalidSignature"),
            ],
        }),
    });
    let if_ = |cond: Expr, then_branch: Expr, else_branch: Expr| {
        sp_expr(ExprNode::If { cond, then_branch, else_branch })
    };

    // def self.find_by_password_reset_token(token)
    //   data = …; record = find_by(id: ActiveRecord::TokenFor.data_id(data))
    //   if record.nil? then nil elsif <still matches> then record else nil end
    let find_by = send(
        None,
        "find_by",
        vec![sp_expr(ExprNode::Hash {
            entries: vec![(sp_expr(ExprNode::Lit { value: Literal::Sym { value: Symbol::from("id") } }), id_of_data())],
            kwargs: true,
        })],
    );
    let lenient = sp_expr(ExprNode::Seq {
        exprs: vec![
            verify(),
            assign("record", find_by),
            if_(
                send(Some(var("record")), "nil?", vec![]),
                nil(),
                if_(still_matches(), var("record"), nil()),
            ),
        ],
    });
    push_synth_class_method(
        methods,
        model,
        Symbol::from(format!("find_by_{}_reset_token", attr.as_str())),
        vec![Param::positional(token.clone())],
        lenient,
        fn_sig(vec![(token.clone(), Ty::Str)], Ty::Union { variants: vec![self_ty.clone(), Ty::Nil] }),
    );

    // The bang form, Rails' `find_by_token_for!`: a token that does not
    // verify, or whose salt no longer matches, is InvalidSignature; one
    // that verifies but names no row is `find`'s RecordNotFound.
    //   data = …; raise InvalidSignature if data == ""
    //   record = find(ActiveRecord::TokenFor.data_id(data))
    //   raise InvalidSignature unless <still matches>
    //   record
    let empty = sp_expr(ExprNode::Lit { value: Literal::Str { value: String::new() } });
    let strict = sp_expr(ExprNode::Seq {
        exprs: vec![
            verify(),
            if_(send(Some(var("data")), "==", vec![empty]), invalid(), nil()),
            assign("record", send(None, "find", vec![id_of_data()])),
            if_(still_matches(), nil(), invalid()),
            var("record"),
        ],
    });
    push_synth_class_method(
        methods,
        model,
        Symbol::from(format!("find_by_{}_reset_token!", attr.as_str())),
        vec![Param::positional(token.clone())],
        strict,
        fn_sig(vec![(token, Ty::Str)], self_ty),
    );
}

/// Rails' default reset-token lifetime, `15.minutes`, in seconds — also
/// the third line of the token's purpose.
const RESET_TOKEN_TTL: i64 = 900;

/// Whether the declaration takes the default `reset_token: true` —
/// absent, or written out as `true`.
fn default_reset_token(body: &[ModelBodyItem]) -> bool {
    body.iter().all(|item| {
        let ModelBodyItem::Unknown { expr, .. } = item else { return true };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else { return true };
        if method.as_str() != "has_secure_password" {
            return true;
        }
        args.iter().all(|a| {
            let ExprNode::Hash { entries, .. } = &*a.node else { return true };
            entries.iter().all(|(k, v)| {
                !matches!(&*k.node, ExprNode::Lit { value: Literal::Sym { value } } if value.as_str() == "reset_token")
                    || matches!(&*v.node, ExprNode::Lit { value: Literal::Bool { value: true } })
            })
        })
    })
}

/// Push a synthesized class method unless the model defines one of
/// that name (custom methods win) or an earlier synthesizer claimed it.
fn push_synth_class_method(
    methods: &mut Vec<MethodDef>,
    model: &Model,
    name: Symbol,
    params: Vec<Param>,
    body: Expr,
    signature: Ty,
) {
    let defined = model.methods().any(|m| m.receiver == MethodReceiver::Class && m.name == name)
        || methods.iter().any(|m| m.receiver == MethodReceiver::Class && m.name == name);
    if defined {
        return;
    }
    methods.push(MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: Span::synthetic(),
        name,
        receiver: MethodReceiver::Class,
        params,
        body,
        signature: Some(signature),
        effects: crate::effect::EffectSet::default(),
        enclosing_class: Some(model.name.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    });
}

fn send(recv: Option<Expr>, method: &str, args: Vec<Expr>) -> Expr {
    sp_expr(ExprNode::Send {
        recv,
        method: Symbol::from(method),
        args,
        block: None,
        parenthesized: true,
    })
}

/// A model's own `<attr>=` that calls `super`. In Rails the macro's
/// writer lives in a module the model includes, so `super` reaches it.
/// Here the model's writer wins outright (`push_secure_password_methods`
/// skips the name), and its `super` went to `ActiveRecord::Base`, which
/// has no such writer: NoMethodError at run time, with `check` clean.
/// This pass adds the macro's writer to the model under a name of its
/// own and makes the override call that where it wrote `super`, as
/// `lower::as_json_super` does for `as_json`.
///
/// A post-analyze pass over the app, ahead of the passes that inline
/// blocks (`create_block`), so it sees the writer as written and every
/// place another writer could come from. Left unchanged, rather than
/// risk skipping a writer or passing the wrong value:
/// - a model with a module mixed into it or an ancestor model, in the
///   class body or by an initializer (`User.include Hooks`): the module
///   could define `<attr>=` too, and `super` would reach it first;
/// - an app that already has a method of the helper's name: in a model
///   (a `def`, an association, a macro such as `attr_reader`), a column
///   of any table, or a library class or module;
/// - a writer whose parameters are not one plain positional, which a
///   bare `super` could not forward as the helper's one argument;
/// - a writer with a `super` inside a block: there the writer's
///   parameter name can mean the block's own variable (a block
///   parameter, or a `|; local|`, which ingest does not keep).
pub fn apply_secure_password_super(app: &mut App) {
    let mut eligible = Vec::new();
    for (i, model) in app.models.iter().enumerate() {
        for attr in secure_password_attrs(&model.body) {
            let helper = helper_name(&attr);
            let lineage = lineage(app, model);
            let left_alone = lineage.iter().any(|m| includes_a_module(m))
                || app.module_mixins.iter().any(|mixin| {
                    mixin.target.as_str() == "ActiveRecord::Base"
                        || lineage.iter().any(|m| m.name.0 == mixin.target)
                })
                || app.models.iter().any(|m| names_method(m, &helper))
                || app.library_classes.iter().any(|lc| lc.methods.iter().any(|m| m.name == helper))
                || app.schema.tables.values().any(|t| t.columns.iter().any(|c| c.name == helper));
            if !left_alone {
                eligible.push((i, attr));
            }
        }
    }
    for (i, attr) in eligible {
        let model = &mut app.models[i];
        let writer = Symbol::from(format!("{}=", attr.as_str()));
        let helper = helper_name(&attr);
        let mut rewritten = false;
        for item in &mut model.body {
            let ModelBodyItem::Method { method, .. } = item else { continue };
            if method.name != writer || method.receiver != MethodReceiver::Instance {
                continue;
            }
            let plain_param = matches!(method.params.as_slice(), [p]
                if p.default.is_none()
                    && !p.keyword
                    && !p.rest
                    && !p.from_keyword
                    && !p.from_kwrest);
            if !plain_param || super_in_a_block(&method.body, false) {
                continue;
            }
            let params: Vec<Symbol> = method.params.iter().map(|p| p.name.clone()).collect();
            if super_to_helper(&mut method.body, &helper, &params) > 0 {
                // It now calls the macro's writer, which writes ivars.
                method.mutates_self = true;
                rewritten = true;
            }
        }
        if rewritten {
            let plain = Symbol::from("unencrypted_password");
            let digest = Symbol::from(format!("{}_digest", attr.as_str()));
            // Typed here: the analyzer has already run.
            let mut body = plaintext_writer_body(&attr, &digest);
            type_writer_body(&mut body, &plain);
            let method = MethodDef {
                visibility: crate::dialect::MethodVisibility::Public,
                unsupported_formals: None,
                has_anonymous_block: false,
                name_span: Span::synthetic(),
                name: helper,
                receiver: MethodReceiver::Instance,
                params: vec![Param::positional(plain.clone())],
                body,
                signature: Some(fn_sig(vec![(plain, Ty::Str)], Ty::Nil)),
                effects: crate::effect::EffectSet::default(),
                enclosing_class: Some(model.name.0.clone()),
                kind: AccessorKind::Method,
                is_async: false,
                mutates_self: true,
                block_param: None,
            };
            model.body.push(ModelBodyItem::Method {
                method,
                leading_comments: Vec::new(),
                leading_blank_line: true,
            });
        }
    }
}

/// Type a synthesized writer body the way the analyzer would have,
/// since it is built after the analyzer ran and `diagnose` reports an
/// untyped read or call in a model body: the String parameter `param`,
/// `BCrypt::Password` and its `create`, `to_s` and `nil?`.
fn type_writer_body(e: &mut Expr, param: &Symbol) {
    e.node.for_each_child_mut(&mut |c| type_writer_body(c, param));
    let bcrypt = || Ty::Class {
        id: crate::ident::ClassId(Symbol::from("BCrypt::Password")),
        args: vec![],
    };
    e.ty = match &*e.node {
        ExprNode::Var { name, .. } if name == param => Some(Ty::Str),
        ExprNode::Const { .. } => Some(bcrypt()),
        ExprNode::Send { method, .. } => match method.as_str() {
            "create" => Some(bcrypt()),
            "to_s" => Some(Ty::Str),
            "nil?" => Some(Ty::Bool),
            _ => e.ty.clone(),
        },
        ExprNode::Lit { value: Literal::Nil } => Some(Ty::Nil),
        _ => e.ty.clone(),
    };
}

/// `model` and its ancestors among the app's models.
fn lineage<'a>(app: &'a App, model: &'a Model) -> Vec<&'a Model> {
    let mut out = vec![model];
    let mut parent = model.parent.clone();
    while let Some(p) = parent {
        let Some(m) = app.models.iter().find(|m| m.name == p) else { break };
        if out.iter().any(|seen| seen.name == m.name) {
            break;
        }
        out.push(m);
        parent = m.parent.clone();
    }
    out
}

fn helper_name(attr: &Symbol) -> Symbol {
    Symbol::from(format!("_secure_{}_writer", attr.as_str()))
}

fn includes_a_module(model: &Model) -> bool {
    model.body.iter().any(|item| {
        matches!(item, ModelBodyItem::Unknown { expr, .. }
            if matches!(&*expr.node, ExprNode::Send { recv: None, method, .. }
                if matches!(method.as_str(), "include" | "prepend")))
    })
}

/// Does `model` name a method `name`: a `def`, an association, or a
/// symbol or call by that name anywhere in its class body
/// (`attr_reader :name`, `alias_method :name, …`, `define_method(:name)`)?
fn names_method(model: &Model, name: &Symbol) -> bool {
    fn mentions(e: &Expr, name: &Symbol) -> bool {
        let here = match &*e.node {
            ExprNode::Lit { value: Literal::Sym { value } } => value == name,
            ExprNode::Send { method, .. } => method == name,
            _ => false,
        };
        let mut found = here;
        e.node.for_each_child(&mut |c| found |= mentions(c, name));
        found
    }
    model.associations().any(|a| a.name() == name)
        || model.body.iter().any(|item| match item {
            ModelBodyItem::Method { method, .. } => &method.name == name,
            ModelBodyItem::Unknown { expr, .. } => mentions(expr, name),
            _ => false,
        })
}

fn super_in_a_block(e: &Expr, in_block: bool) -> bool {
    if in_block && matches!(&*e.node, ExprNode::Super { .. }) {
        return true;
    }
    let inside = in_block || matches!(&*e.node, ExprNode::Lambda { .. });
    let mut found = false;
    e.node.for_each_child(&mut |c| found |= super_in_a_block(c, inside));
    found
}

/// Replace each `super` in `e` with `self.<helper>(…)`, innermost
/// first, and count them. A bare `super` passes the writer's own
/// `params`.
fn super_to_helper(e: &mut Expr, helper: &Symbol, params: &[Symbol]) -> usize {
    let mut count = 0;
    e.node.for_each_child_mut(&mut |c| count += super_to_helper(c, helper, params));
    let ExprNode::Super { args } = &*e.node else { return count };
    // A bare `super` forwards the writer's parameter, which the helper
    // takes as its String plaintext.
    let args = args.clone().unwrap_or_else(|| {
        params
            .iter()
            .map(|p| {
                let mut read = sp_expr(ExprNode::Var { id: VarId(0), name: p.clone() });
                read.ty = Some(Ty::Str);
                read
            })
            .collect()
    });
    *e.node = ExprNode::Send {
        recv: Some(sp_expr(ExprNode::SelfRef)),
        method: helper.clone(),
        args,
        block: None,
        parenthesized: true,
    };
    count + 1
}

/// Rails names the authenticator after the attribute, except the
/// default `password` which gets the bare `authenticate`. One home,
/// because `authenticate_by`'s call-site expansion has to name the same
/// method this pass synthesizes.
pub(crate) fn authenticator_name(attr: &Symbol) -> Symbol {
    if attr.as_str() == "password" {
        Symbol::from("authenticate")
    } else {
        Symbol::from(format!("authenticate_{}", attr.as_str()))
    }
}

/// The secure-password attribute name when the model body declares
/// `has_secure_password` (first positional symbol, default
/// `password`), else None. Mirrors analyze's registration scan.
/// `pub(crate)` for the permit-writer filter (model_to_library), which
/// counts the synthesized plaintext writers as assignable.
pub(crate) fn secure_password_attr(body: &[ModelBodyItem]) -> Option<Symbol> {
    secure_password_attrs(body).into_iter().next()
}

/// Every secure-password attribute the model declares, in declaration
/// order — Rails allows more than one `has_secure_password` per model,
/// and `authenticate_by` partitions its keyword arguments against the
/// whole set (a key naming one of these is a password to check; every
/// other key is a finder condition).
pub(crate) fn secure_password_attrs(body: &[ModelBodyItem]) -> Vec<Symbol> {
    let mut out = Vec::new();
    for item in body {
        let ModelBodyItem::Unknown { expr, .. } = item else {
            continue;
        };
        let ExprNode::Send { recv: None, method, args, .. } = &*expr.node else {
            continue;
        };
        if method.as_str() != "has_secure_password" {
            continue;
        }
        let attr = args
            .iter()
            .find_map(|a| match &*a.node {
                ExprNode::Lit { value: Literal::Sym { value } } => Some(value.clone()),
                _ => None,
            })
            .unwrap_or_else(|| Symbol::from("password"));
        if !out.contains(&attr) {
            out.push(attr);
        }
    }
    out
}

fn sp_expr(node: ExprNode) -> Expr {
    Expr::new(Span::synthetic(), node)
}

fn ivar_read(name: &Symbol) -> Expr {
    sp_expr(ExprNode::Ivar { name: name.clone() })
}

fn plain_ivar_assign(name: &Symbol, param: &Symbol) -> Expr {
    sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: name.clone() },
        value: sp_expr(ExprNode::Var { id: VarId(0), name: param.clone() }),
    })
}

/// `BCrypt::Password.new(@<digest>) == unencrypted_password ? self : false`.
fn authenticate_body(digest: &Symbol) -> Expr {
    let wrapped = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const {
            path: vec![Symbol::from("BCrypt"), Symbol::from("Password")],
        })),
        method: Symbol::from("new"),
        args: vec![ivar_read(digest)],
        block: None,
        parenthesized: true,
    });
    let cmp = sp_expr(ExprNode::Send {
        recv: Some(wrapped),
        method: Symbol::from("=="),
        args: vec![sp_expr(ExprNode::Var {
            id: VarId(0),
            name: Symbol::from("unencrypted_password"),
        })],
        block: None,
        parenthesized: false,
    });
    sp_expr(ExprNode::If {
        cond: cmp,
        then_branch: sp_expr(ExprNode::SelfRef),
        else_branch: sp_expr(ExprNode::Lit { value: Literal::Bool { value: false } }),
    })
}

/// The plaintext writer Rails' macro provides:
///   `@<attr> = v; @<attr>_digest = BCrypt::Password.create(v).to_s unless v.nil?`
/// (`.to_s` because BCrypt::Password subclasses String but the digest
/// column stores plain text). Nil skips digest generation, mirroring
/// Rails' blank-guard closely enough for the login/rehash paths.
fn plaintext_writer_body(attr: &Symbol, digest: &Symbol) -> Expr {
    let value_var = || {
        sp_expr(ExprNode::Var { id: VarId(0), name: Symbol::from("unencrypted_password") })
    };
    let store_plain = sp_expr(ExprNode::Assign {
        target: LValue::Ivar { name: attr.clone() },
        value: value_var(),
    });
    let create = sp_expr(ExprNode::Send {
        recv: Some(sp_expr(ExprNode::Const {
            path: vec![Symbol::from("BCrypt"), Symbol::from("Password")],
        })),
        method: Symbol::from("create"),
        args: vec![value_var()],
        block: None,
        parenthesized: true,
    });
    let digest_str = sp_expr(ExprNode::Send {
        recv: Some(create),
        method: Symbol::from("to_s"),
        args: Vec::new(),
        block: None,
        parenthesized: false,
    });
    let guarded_digest = sp_expr(ExprNode::If {
        cond: sp_expr(ExprNode::Send {
            recv: Some(value_var()),
            method: Symbol::from("nil?"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        }),
        then_branch: sp_expr(ExprNode::Lit { value: Literal::Nil }),
        else_branch: sp_expr(ExprNode::Assign {
            target: LValue::Ivar { name: digest.clone() },
            value: digest_str,
        }),
    });
    sp_expr(ExprNode::Seq { exprs: vec![store_plain, guarded_digest] })
}
