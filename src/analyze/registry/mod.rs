//! Post-model-loop class-registry population, split out of
//! [`super::Analyzer::with_adapter`] by domain. Each submodule owns one
//! framework/stdlib surface and takes `&mut HashMap<ClassId, ClassInfo>`
//! (plus whatever the domain needs explicitly). Pure code motion — the
//! orchestrator in `with_adapter` calls them in the original emit order.

pub(super) mod activemodel;
pub(super) mod ar;
pub(super) mod library;
pub(super) mod routes;
pub(super) mod stdlib;
pub(super) mod view;

use crate::effect::EffectSet;
use crate::ty::Ty;

/// A block-yielding `Fn` type — `() { (block_ty) -> ret } -> ret` — used to
/// register `form_with` / `fields_for` / `respond_to` and friends so the
/// yielded block param types. Shared by the view and controller
/// registrations (both once defined this inline).
pub(in crate::analyze) fn block_fn(block_ty: &Ty, ret: Ty) -> Ty {
    Ty::Fn {
        params: vec![],
        block: Some(Box::new(block_ty.clone())),
        ret: Box::new(ret),
        effects: EffectSet::default(),
    }
}
