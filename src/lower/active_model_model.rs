//! `include ActiveModel::Model` on a plain class — synthesized, not
//! provided.
//!
//! Rails' `ActiveModel::Model` (an alias for `ActiveModel::API`) mixes
//! six modules into a class that has no table: attribute assignment,
//! validations, conversion, naming, translation, access. Almost all of
//! that is machinery for something else; what a class *including* it
//! actually gains, at the call sites our corpus has, is three methods:
//!
//!   * `initialize(attributes = {})` — assign each key through the
//!     matching public writer;
//!   * `valid?` — run the declared validations (none, in a class that
//!     declares none, so: `true`);
//!   * `persisted?` — `false`, the whole point of a tableless model.
//!
//! Two ways to give a class those. Ship `ActiveModel::Model` as a
//! runtime module and leave the `include` standing, or synthesize the
//! three methods into the class and drop the `include`. This pass does
//! the second, because the runtime module can only assign through
//! `public_send("#{name}=", value)` — a dispatch on a computed name,
//! which the CRuby overlay serves and no strict target can. The
//! synthesized form names each ivar literally, so every target compiles
//! it, and the emitted class says what it got instead of pointing at a
//! module the reader has to go read.
//!
//! The writer list IS the contract, not a convenience: Rails assigns
//! through public writers and raises `UnknownAttributeError` for a key
//! with none, so a class's `attr_accessor`/`attr_writer` surface (which
//! ingest has already lowered to `AttributeWriter` methods) is exactly
//! the set of keys `new` accepts.
//!
//! WHY LIBRARY CLASSES ONLY. A superclass-less class under
//! `app/models/` that includes this is classified `ClassKind::Model` at
//! ingest (`library_class::includes_active_model`) and rides the
//! tableless-model path, which lowers its `validates` DSL into a real
//! `valid?`/`errors` pair. That path is strictly better and already
//! exists; this one covers the classes it cannot see — campfire's
//! `ActionText::Attachment::OpengraphEmbed` lives in `lib/rails_ext/`,
//! so it is a library class no matter what it includes, and the
//! surviving `include` was a `NameError` at load time.
//!
//! DECLINES, each with a `lower_residue` entry naming the reason:
//!   * the class defines its own `initialize` — Rails' would be
//!     overridden anyway, but then `valid?`/`persisted?` are the only
//!     gain and dropping the `include` silently would be a guess about
//!     what else the module was there for;
//!   * the class declares a validation (`validate` method or a
//!     `validates` class-body call) — a synthesized `valid? => true`
//!     would answer *yes* for a record the app wrote rules to reject,
//!     which is the one failure mode worth refusing outright;
//!   * the class has no attribute writers — nothing to assign, so the
//!     `include` is doing something this pass does not model.

use crate::app::App;
use crate::diagnostic::Diagnostic;
use crate::dialect::{AccessorKind, LibraryClass, MethodDef, MethodReceiver, Param};
use crate::effect::EffectSet;
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ident::{ClassId, Symbol, VarId};
use crate::span::Span;
use crate::ty::{Param as TyParam, ParamKind, Ty};

const MODULE: &str = "ActiveModel::Model";

pub fn apply_active_model_model_synthesis(app: &mut App) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    for lc in &mut app.library_classes {
        if !lc.includes.iter().any(|i| i.0.as_str() == MODULE) {
            continue;
        }
        if let Some(reason) = decline_reason(lc) {
            diags.push(crate::lower::residue_diagnostic(
                "active_model_model",
                "include ActiveModel::Model",
                Span::synthetic(),
                reason,
                format!(
                    "`{}` includes `{MODULE}`, left standing ({reason}) — the module has no \
                     definition in any emitted tree, so the class raises NameError at load time",
                    lc.name.0.as_str()
                ),
            ));
            continue;
        }
        let writers = attribute_writers(&lc.methods);
        let owner = lc.name.clone();
        // Analysis skips a writer with no call site. These writers are
        // that case: the only assignment is the constructor this pass
        // is about to add, and the value is the untyped hash slot.
        stamp_untyped_attr_writers(&mut lc.methods);
        lc.methods.push(attributes_initialize(&owner, Span::synthetic(), &writers));
        if !defines(lc, "valid?") {
            lc.methods.push(constant_predicate(&owner, "valid?", true));
        }
        if !defines(lc, "persisted?") {
            lc.methods.push(constant_predicate(&owner, "persisted?", false));
        }
        lc.includes.retain(|i| i.0.as_str() != MODULE);
    }
    diags
}

/// `None` when the class can be synthesized; otherwise the reason the
/// ledger records.
fn decline_reason(lc: &LibraryClass) -> Option<&'static str> {
    if defines(lc, "initialize") {
        return Some("class defines its own initialize");
    }
    if defines(lc, "validate") || declares_validates(lc) {
        return Some("class declares validations, which this pass does not lower");
    }
    if attribute_writers(&lc.methods).is_empty() {
        return Some("class declares no attribute writers to assign through");
    }
    None
}

fn defines(lc: &LibraryClass, name: &str) -> bool {
    lc.methods
        .iter()
        .any(|m| m.receiver == MethodReceiver::Instance && m.name.as_str() == name)
}

/// A `validates …` / `validate :sym` call captured in the class body.
/// Ingest parks any class-body call it does not model in `unknown_calls`
/// as real IR, which is what makes this checkable rather than a guess.
fn declares_validates(lc: &LibraryClass) -> bool {
    lc.unknown_calls.iter().any(|e| {
        matches!(&*e.node, ExprNode::Send { recv: None, method, .. }
            if matches!(method.as_str(), "validates" | "validate" | "validates_each"))
    })
}

/// Fill an `attr_accessor` / `attr_writer` analysis left unsigned.
///
/// `stamp_inferred_method_signatures` skips a writer when no call site
/// taught it a value type. That is the honest type of this declaration:
/// `attr_accessor` names no column, so the param is `untyped`. A writer
/// analysis already signed (`attr_writer id: String`) is left alone.
fn stamp_untyped_attr_writers(methods: &mut [MethodDef]) {
    for method in methods {
        if method.kind != AccessorKind::AttributeWriter || method.signature.is_some() {
            continue;
        }
        let Some(param) = method.params.first() else {
            continue;
        };
        let value_ty = Ty::Untyped;
        if let ExprNode::Assign { value, .. } = &mut *method.body.node {
            if value.ty.is_none() {
                value.ty = Some(value_ty.clone());
            }
        }
        method.signature = Some(Ty::Fn {
            params: vec![TyParam {
                name: param.name.clone(),
                ty: value_ty.clone(),
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(value_ty),
            effects: EffectSet::default(),
        });
    }
}

/// The names a `attr_writer` / `attr_accessor` declared, in declaration
/// order. Ingest tags those `AccessorKind::AttributeWriter`, so this
/// reads the tag rather than re-deriving the shape from the body.
fn attribute_writers(methods: &[MethodDef]) -> Vec<Symbol> {
    let mut out = Vec::new();
    for m in methods {
        if m.receiver != MethodReceiver::Instance || m.kind != AccessorKind::AttributeWriter {
            continue;
        }
        let name = m.name.as_str();
        let Some(base) = name.strip_suffix('=') else { continue };
        let base = Symbol::from(base);
        if !out.contains(&base) {
            out.push(base);
        }
    }
    out
}

/// `def initialize(attrs = {}) @href = attrs[:href]; … end` — the
/// constructor `ActiveModel::Model` supplies, built once here and
/// shared with the model-side twin
/// (`model_to_library::validations::push_active_model_constructor`),
/// which reaches the same shape from a `Model`'s `attr_*` declarations
/// instead of a `LibraryClass`'s lowered writers.
///
/// Symbol keys, and only symbol keys: every call site in the corpus
/// builds the hash as a literal with symbol keys. Rails' own path goes
/// through `assign_attributes`, which stringifies and would accept
/// both; accepting both here would mean a lookup per key with a
/// fallback — two reads of a hash that has one spelling. A string-keyed
/// caller belongs here as a deliberate change, not as a shape guessed
/// at in advance.
pub(crate) fn attributes_initialize(
    owner: &ClassId,
    span: Span,
    names: &[Symbol],
) -> MethodDef {
    let attrs = Symbol::from("attrs");
    // Known by construction, same bag the schema constructor already
    // declares: symbol keys, untyped values. Not inferred from `[]` —
    // String and Array index too. The read is the hash's value type.
    // Index emit turns a missing key into `Value::Null` for an untyped
    // slot, so the field stays `Value` rather than a nilable union that
    // the untyped getter cannot return.
    let attrs_ty = Ty::Hash { key: Box::new(Ty::Sym), value: Box::new(Ty::Untyped) };
    let read_ty = Ty::Untyped;
    let assigns: Vec<Expr> = names
        .iter()
        .map(|name| {
            let mut recv = Expr::new(
                span,
                ExprNode::Var { id: VarId(0), name: attrs.clone() },
            );
            recv.ty = Some(attrs_ty.clone());
            let mut key = Expr::new(
                span,
                ExprNode::Lit { value: Literal::Sym { value: name.clone() } },
            );
            key.ty = Some(Ty::Sym);
            // `attrs[:name]` as the `[]` send every target already
            // lowers — there is no Index node. Receiver and result types
            // are the signature's, stamped here. Index emit reads
            // `recv.ty`; it does not infer a Hash from `[]`.
            let mut read = Expr::new(
                span,
                ExprNode::Send {
                    recv: Some(recv),
                    method: Symbol::from("[]"),
                    args: vec![key],
                    block: None,
                    parenthesized: true,
                },
            );
            read.ty = Some(read_ty.clone());
            let mut assign = Expr::new(
                span,
                ExprNode::Assign { target: LValue::Ivar { name: name.clone() }, value: read },
            );
            assign.ty = Some(read_ty.clone());
            assign
        })
        .collect();
    let mut param = Param::positional(attrs.clone());
    param.default =
        Some(Expr::new(span, ExprNode::Hash { entries: Vec::new(), kwargs: false }));
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Private,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from("initialize"),
        receiver: MethodReceiver::Instance,
        params: vec![param],
        body: Expr::new(span, ExprNode::Seq { exprs: assigns }),
        signature: Some(Ty::Fn {
            params: vec![TyParam {
                name: attrs,
                ty: attrs_ty,
                kind: ParamKind::Optional,
            }],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: EffectSet::default(),
        }),
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: true,
        block_param: None,
    }
}

fn constant_predicate(owner: &ClassId, name: &str, value: bool) -> MethodDef {
    MethodDef {
        visibility: crate::dialect::MethodVisibility::Public,
        unsupported_formals: None,
        has_anonymous_block: false,
        name_span: crate::span::Span::synthetic(),
        name: Symbol::from(name),
        receiver: MethodReceiver::Instance,
        params: Vec::new(),
        body: Expr::new(Span::synthetic(), ExprNode::Lit { value: Literal::Bool { value } }),
        signature: None,
        effects: EffectSet::default(),
        enclosing_class: Some(owner.0.clone()),
        kind: AccessorKind::Method,
        is_async: false,
        mutates_self: false,
        block_param: None,
    }
}
