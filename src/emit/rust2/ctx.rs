//! `EmitCtx` — explicit context object for rust2 emit pipeline globals
//! and per-class scope state.
//!
//! Phase 1 of the EmitCtx refactor bundled the two cross-LC class-method
//! registries (`GLOBAL_CLASS_METHODS` + `GLOBAL_CLASS_METHOD_DEFAULTS`).
//! Phase 2 (this revision) lifts the four per-class thread-locals —
//! `IVAR_TYPES`, `CLASS_METHOD_PARAM_TYS`, `STATIC_METHODS`,
//! `CONSTRUCTOR_FIELDS` — onto the same struct as `RefCell` fields.
//!
//! Storage shape unchanged: `expr/mod.rs::EMIT_CTX` holds an
//! `Option<Rc<EmitCtx>>`; the existing accessor functions read through
//! it; the `with_X<F>(...)` wrappers now swap field values on the
//! installed `EmitCtx` via save-restore rather than on a dedicated
//! per-field thread-local.
//!
//! Why bundle: the registries are exactly what #22's Stage 4
//! (`OPTION_WRAP` + `COERCE_FAMILY`) decide walker needs for callee
//! param-Ty lookup. Without `EmitCtx`, state sat behind a sprawl of
//! parallel thread-locals invisible to non-emit consumers. With
//! `EmitCtx`, downstream decide-pass work takes `&EmitCtx` as an
//! explicit parameter and the per-class state is reachable from
//! anywhere holding an `Rc<EmitCtx>`.
//!
//! Phases 3 and 4 (per-method state and the transient render flags)
//! follow the same pattern; each is its own commit.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::ty::{Param, Ty};

/// Emit-pipeline state. Built once per `rust2::emit` call (in
/// `collect_global_class_methods`) after every category of
/// `LibraryClass` has been assembled. Holds the cross-LC dispatch
/// registry, the parallel kwarg-default registry, and the per-class
/// scope state that used to live as four parallel `RefCell` thread-
/// locals in `expr/mod.rs` (Phase 2 of #24).
///
/// Pipeline-global fields are plain `HashMap`s — read-only after
/// construction. Per-class fields are `RefCell`-wrapped so emit
/// functions can take `&EmitCtx` (not `&mut`) while the `with_X<F>`
/// wrappers in `expr/mod.rs` swap values around each class-scope
/// boundary via save-restore.
#[derive(Debug, Default)]
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

    /// Ivar name → declared field type for the class currently being
    /// emitted. Read by `expr/mod.rs::ivar_field_ty` so `emit_assign`
    /// can coerce mismatched RHS types (canonical case: `self.body =
    /// ""` where `""` is `&str` but the field is `String`). Empty
    /// outside class-body scope; swapped in/out by `with_ivar_types`
    /// at each `impl` block entry.
    pub ivar_types: RefCell<HashMap<String, Ty>>,

    /// Method name → positional param Ty list for the class currently
    /// being emitted. Seeded by `library.rs` via
    /// `with_class_method_param_tys` at class-scope entry; consulted
    /// by the Send arg-coercion walker for `Self::method(args)` so
    /// the callee-back-propagation families can fire on sibling
    /// method calls within the same `impl` block.
    pub class_method_param_tys: RefCell<HashMap<String, Vec<Ty>>>,

    /// Methods in the current `impl` block classified as static-safe
    /// by `library.rs::method_reads_self`. The Send walker routes
    /// implicit-`self` calls into these as `Self::method(args)` so
    /// they compile inside `pub fn new` (no instance yet) and read
    /// as the cleaner Rust form generally.
    pub static_methods: RefCell<HashSet<String>>,

    /// Field names of the struct being constructed by the currently-
    /// emitting `pub fn new`. Empty outside constructor scope. The
    /// `Return { Nil }` arm reads this to materialize `return Self {
    /// f1, f2 }` for Ruby `return if cond` early exits — without it,
    /// the constructor early-exits with bare `return` and the type
    /// system rejects.
    pub constructor_fields: RefCell<Vec<String>>,
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
