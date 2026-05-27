//! `EmitCtx` — explicit context object for rust2 emit pipeline globals.
//!
//! Phase 1 of the EmitCtx refactor (separate from #22's decide/render
//! split). Bundles the two cross-LC class-method registries that used
//! to live as the `GLOBAL_CLASS_METHODS` + `GLOBAL_CLASS_METHOD_DEFAULTS`
//! thread-locals in `expr/mod.rs`. The thread-local storage shape
//! stays — `expr/mod.rs::EMIT_CTX` holds an `Option<Rc<EmitCtx>>` and
//! the existing accessor functions read through it — but the data is
//! now an explicit struct that can be passed by reference to non-emit
//! consumers like the decide pass.
//!
//! Why bundle: the registry is exactly what #22's Stage 4
//! (`OPTION_WRAP` + `COERCE_FAMILY`) decide walker needs for callee
//! param-Ty lookup. Without `EmitCtx`, the registry sits behind a
//! thread-local that's only populated late in `emit()` (inside
//! `with_global_class_methods`), making it unavailable to the decide
//! pass that runs earlier per-category. With `EmitCtx`, downstream
//! Stage 4 work can take `&EmitCtx` as an explicit parameter and
//! run wherever the registry is built.
//!
//! Subsequent phases of the EmitCtx refactor (per-class, per-method,
//! per-expression state) lift the rest of the thread-locals in
//! `expr/mod.rs` into this same struct. Each phase is its own
//! commit; this one establishes the struct and migrates only the
//! pipeline globals.

use std::collections::HashMap;

use crate::ty::{Param, Ty};

/// Pipeline-global emit state. Built once per `rust2::emit` call
/// (in `collect_global_class_methods`) after every category of
/// `LibraryClass` has been assembled. Holds the cross-LC dispatch
/// registry and the parallel kwarg-default registry.
///
/// Read-only after construction; both fields are plain `HashMap`s,
/// not `RefCell`-wrapped, because no emit code mutates them once
/// they're built. The `expr/mod.rs::EMIT_CTX` thread-local wraps
/// the struct in an `Option` and a save-restore pattern around the
/// emit loop preserves the previous value (matching the prior
/// `with_global_class_methods` contract).
#[derive(Clone, Debug, Default)]
pub struct EmitCtx {
    /// Cross-LC class-method registry: `(ClassName, method) →
    /// Vec<Param>`. Populated from every LC's `MethodDef.signature`
    /// (when typed) or `MethodDef.params` (untyped fallback).
    /// Block / KeywordRest params are filtered out at collection time
    /// so positional indices align with what the Const-recv dispatch
    /// arm expects.
    pub global_class_methods: HashMap<String, HashMap<String, Vec<Param>>>,

    /// Parallel registry of per-position pre-rendered kwarg defaults.
    /// `Some(rendered)` for literal-shape defaults the Const-recv
    /// dispatch can substitute when a kwarg is unsupplied; `None`
    /// for positional non-default params and complex defaults.
    /// Positions align 1:1 with `global_class_methods` by
    /// `(class, method)`.
    pub global_class_method_defaults:
        HashMap<String, HashMap<String, Vec<Option<String>>>>,
}

impl EmitCtx {
    /// Cross-class lookup: `(ClassName, method) → Vec<Ty>`. Returns
    /// the positional-param Ty list for a callee in a different LC
    /// than the currently-emitting class. Mirrors the historical
    /// `global_class_method_param_tys` thread-local accessor.
    pub fn lookup_param_tys(&self, class: &str, method: &str) -> Option<Vec<Ty>> {
        self.global_class_methods
            .get(class)
            .and_then(|methods| methods.get(method))
            .map(|params| params.iter().map(|p| p.ty.clone()).collect())
    }

    /// Rich variant returning the full `Param` list (name + ty +
    /// kind). Mirrors `global_class_method_params`.
    pub fn lookup_params(&self, class: &str, method: &str) -> Option<Vec<Param>> {
        self.global_class_methods
            .get(class)
            .and_then(|methods| methods.get(method).cloned())
    }

    /// Per-position kwarg-default lookup. Mirrors
    /// `global_class_method_param_default`.
    pub fn lookup_param_default(
        &self,
        class: &str,
        method: &str,
        idx: usize,
    ) -> Option<String> {
        self.global_class_method_defaults
            .get(class)
            .and_then(|methods| methods.get(method))
            .and_then(|defaults| defaults.get(idx).cloned())
            .flatten()
    }
}
