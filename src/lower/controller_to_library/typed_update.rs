//! #856: a typed `update_from_<x>_params` exists only when a model is
//! named after the resource and has a table. Otherwise Rails' `update`
//! / `update!` (and a `new` / `create` left holding a params object)
//! take the attribute Hash, `to_attrs`.
//!
//! Split out of `params` the way `permit_splat` was. Permit recognition
//! and the `to_attrs` method body stay there; this module owns when the
//! typed variant applies and the late demand that grows `to_attrs`.

use std::collections::{BTreeSet, HashMap};

use crate::dialect::{LibraryClass, MethodDef};
use crate::expr::{Expr, ExprNode, Literal};
use crate::ident::{ClassId, Symbol};
use crate::ty::Ty;

use super::params::{ParamsSpec, ParamsSpecs, body_tail, expr, synth_to_attrs};

/// Record which resources name a model with a table: only those get
/// the typed update variants (`model_to_library`'s
/// `push_update_typed_variants`). An empty model list leaves the
/// set unknown.
pub(super) fn mark_model_resources(
    specs: &mut ParamsSpecs,
    models: &[crate::dialect::Model],
    schema: Option<&crate::schema::Schema>,
) {
    let Some(schema) = schema else { return };
    if models.is_empty() {
        return;
    }
    specs.model_resources = Some(
        models
            .iter()
            .filter(|m| schema.tables.contains_key(&m.table.0))
            .map(|m| Symbol::from(crate::naming::snake_case(m.name.0.as_str())))
            .collect(),
    );
}

/// Whether some model defines `spec`'s typed update variants (true
/// when the models are not known).
pub(super) fn has_typed_update(specs: &ParamsSpecs, spec: &ParamsSpec) -> bool {
    specs
        .model_resources
        .as_ref()
        .is_none_or(|set| set.contains(&spec.resource))
}

/// Each method's name with the type of the value its typed body
/// answers (its last statement's).
pub(super) fn helper_answers(methods: &[MethodDef]) -> HashMap<Symbol, Option<Ty>> {
    methods
        .iter()
        .map(|m| (m.name.clone(), body_tail(&m.body).ty.clone()))
        .collect()
}

/// `record.update(x)` / `update!(x)` where `x` is TYPED as a params
/// class no model has a typed update variant for (#856): Rails' `update`
/// takes the attribute Hash, which is `x.to_attrs`. Runs on a typed
/// body, so a helper that answers a Hash (`permitted.to_h.except(:k)`)
/// is left as written, and the class is the one the call site holds
/// (a controller-qualified list, not the resource's canonical one).
/// Each class read this way is added to `wanted`.
///
/// `answers` is each of the controller's own methods' typed body value
/// ([`helper_answers`]): a call to one of them counts only when its body
/// answers the params class, whatever its signature declares.
pub(super) fn hand_update_the_attrs(
    body: &mut Expr,
    specs: &ParamsSpecs,
    answers: &HashMap<Symbol, Option<Ty>>,
    wanted: &mut BTreeSet<ClassId>,
) {
    body.node
        .for_each_child_mut(&mut |c| hand_update_the_attrs(c, specs, answers, wanted));
    let ExprNode::Send {
        recv: Some(recv),
        method,
        args,
        block: None,
        ..
    } = &mut *body.node
    else {
        return;
    };
    // `Model.new(x)` / `create(x)` / `create!(x)` take the attribute Hash
    // too. The typed factories are reached by name before typing
    // (`rewrite_model_new_to_from_params`), so one left here holds the
    // params object some other way: a local (`attrs = <permit
    // chain>.reverse_merge(...)`) or the statements a defaults merge
    // lowers to.
    let constructs = matches!(method.as_str(), "new" | "create" | "create!")
        && matches!(&*recv.node, ExprNode::Const { .. });
    let updates = matches!(method.as_str(), "update" | "update!");
    if !(constructs || updates) || args.len() != 1 {
        return;
    }
    let Some(Ty::Class { id, .. }) = args[0].ty.as_ref() else {
        return;
    };
    let Some(spec) = specs.by_class(id) else {
        return;
    };
    if let ExprNode::Send {
        recv,
        method: helper,
        args: helper_args,
        block: None,
        ..
    } = &*args[0].node
    {
        let own = recv
            .as_ref()
            .is_none_or(|r| matches!(&*r.node, ExprNode::SelfRef))
            && helper_args.is_empty();
        if own {
            if let Some(answer) = answers.get(helper) {
                if !matches!(answer, Some(Ty::Class { id: a, .. }) if a == id) {
                    return;
                }
            }
        }
    }
    if updates && has_typed_update(specs, spec) {
        return;
    }
    let class_id = spec.class_id.clone();
    let value = std::mem::replace(
        &mut args[0],
        expr(
            ExprNode::Lit {
                value: Literal::Nil,
            },
            None,
        ),
    );
    let span = value.span;
    let mut attrs = expr(
        ExprNode::Send {
            recv: Some(value),
            method: Symbol::from("to_attrs"),
            args: Vec::new(),
            block: None,
            parenthesized: false,
        },
        Some(Ty::Hash {
            key: std::sync::Arc::new(Ty::Sym),
            value: std::sync::Arc::new(Ty::Untyped),
        }),
    );
    attrs.span = span;
    args[0] = attrs;
    wanted.insert(class_id);
}

/// After every body of one controller is typed, rewrite the calls that
/// still hold a params object and remember which classes must answer
/// `to_attrs`.
pub(super) fn record_to_attrs_demand(
    methods: &mut [MethodDef],
    specs: &ParamsSpecs,
    wanted: &mut BTreeSet<ClassId>,
) {
    let answers = helper_answers(methods);
    for method in methods {
        hand_update_the_attrs(&mut method.body, specs, &answers, wanted);
    }
}

/// Grow `to_attrs` on each demanded params class that synthesis did not
/// already give one (early demand sets `ParamsSpec::wants_to_attrs`).
pub(super) fn synthesize_demanded_to_attrs(
    params_lcs: &mut [LibraryClass],
    specs: &ParamsSpecs,
    wanted: &BTreeSet<ClassId>,
) {
    for params_lc in params_lcs {
        if wanted.contains(&params_lc.name)
            && !params_lc
                .methods
                .iter()
                .any(|m| m.name.as_str() == "to_attrs")
        {
            if let Some(spec) = specs.by_class(&params_lc.name) {
                params_lc
                    .methods
                    .push(synth_to_attrs(&spec.class_id, &spec.fields));
            }
        }
    }
}
