//! Post-model-loop class-registry population, split out of
//! [`super::Analyzer::with_adapter`] by domain. Each submodule owns one
//! framework/stdlib surface and takes `&mut HashMap<ClassId, ClassInfo>`
//! (plus whatever the domain needs explicitly). Pure code motion — the
//! orchestrator in `with_adapter` calls them in the original emit order.

pub(super) mod activemodel;
pub(super) mod stdlib;
