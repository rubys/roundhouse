//! `EmitCtx` — explicit context object for rust2 emit state.
//!
//! Phase 1 bundled the two cross-LC class-method registries
//! (`GLOBAL_CLASS_METHODS` + `GLOBAL_CLASS_METHOD_DEFAULTS`).
//! Phase 2 lifted the four per-class thread-locals (`IVAR_TYPES`,
//! `CLASS_METHOD_PARAM_TYS`, `STATIC_METHODS`, `CONSTRUCTOR_FIELDS`).
//! Phase 3 lifted the seven per-method thread-locals (`MUT_VARS`,
//! `DECLARED_VARS`, `BACK_PROPAGATED_HASH_LOCALS`, `PARAM_TYPES`,
//! `LOCAL_VAR_TYPES`, `REBOUND_VARS`, `CURRENT_RETURN_TY`).
//! Phase 4 (this revision) lifts the five transient render flags —
//! `IN_CONSTRUCTOR`, `IN_CLASS_METHOD`, `IN_RETURN_TAIL`,
//! `IN_MODULE_SINGLETON`, `SUPPRESS_VAR_CLONE` — as `Cell<bool>`
//! fields. Cheap get/set, no `RefCell` overhead.
//!
//! Storage shape unchanged: `expr/mod.rs::EMIT_CTX` holds an
//! `Option<Rc<EmitCtx>>`; the existing accessor functions read through
//! it; the `with_X<F>(...)` wrappers swap field values on the
//! installed `EmitCtx` via save-restore rather than on dedicated
//! per-field thread-locals.
//!
//! After Phase 4 the only thread-local left in `expr/mod.rs` is
//! `EMIT_CTX` itself — the install slot. Every other emit state
//! lives on `EmitCtx` and is reachable from any code holding an
//! `Rc<EmitCtx>`.

use std::cell::{Cell, RefCell};
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

    /// Program-global set of method names flagged `mutates_self`
    /// (any class). Read by the `recv.each { |x| … }` → `iter_mut`
    /// bridge in `expr/mod.rs` to tell a *mutating* block (the
    /// `_preload_<assoc>` distribute loop) from a read-only render
    /// loop: a mutating block must NOT clone its receiver, or the
    /// `&mut`-method writes land on a throwaway temporary and are
    /// silently dropped (roundhouse#40 — the eager-load was dead on
    /// Rust). Name-keyed (not (class,method)) because the bridge sees
    /// the block param's call site, not its resolved class; an
    /// over-match would at worst suppress a clone the borrow checker
    /// would then flag loudly, never miscompile silently.
    pub global_mutating_methods: HashSet<String>,

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

    /// Variable names that the current method body assigns more than
    /// once. Pre-computed by `with_method_scope` (a one-shot walker
    /// counts `Assign LValue::Var` sites + Var-Send-receiver uses).
    /// First-assignment site emits `let mut name = expr`; later sites
    /// emit plain `name = expr` (rebind, no shadow). Single-assignment
    /// locals stay immutable: `let name = expr`. Without this, Ruby
    /// `i = 0; while ...; i += 1; end` would shadow `i` inside the
    /// loop and loop forever.
    pub mut_vars: RefCell<HashSet<String>>,

    /// Variable names the current method body has already emitted a
    /// `let` binding for. Subsequent `Assign LValue::Var` sites for
    /// the same name rebind without re-declaring. Scoped per-branch
    /// by `with_declared_vars_scope` so an `If` branch's locals don't
    /// leak into the sibling branch or out past the `If`.
    pub declared_vars: RefCell<HashSet<String>>,

    /// Variable names whose `local_var_ty` was set from the back-
    /// propagated function-return type (`empty_hash_return_ty`), not
    /// from the value's body-typer `Ty`. The Send `[]=` peephole uses
    /// this to know the recorded type is authoritative — for body-
    /// typer-derived `r.ty` the storage may disagree (e.g. `Hash<Sym,
    /// Str>` in IR but `HashMap<&str, String>` in emit).
    pub back_propagated_hash_locals: RefCell<HashSet<String>>,

    /// Parameter name → declared RBS type for the enclosing method,
    /// set by `method.rs` around each method-body emit. The body-typer
    /// doesn't always propagate the param's Option-ness to Var reads,
    /// so `emit_assign`'s String coercion needs this side channel to
    /// avoid `.to_string()`-ing an `Option<String>`-typed param
    /// reference (which fails Display).
    pub param_types: RefCell<HashMap<String, Ty>>,

    /// Per-Seq tracking of local-var declared types. Populated by
    /// `Assign { LValue::Var, value }` sites with known `value.ty`.
    /// Read by the narrowing-aware Var emit so a local `params =
    /// match_pattern(...)` (Option<HashMap>) participates in the same
    /// narrowing+unwrap dance as an Option-typed function param.
    /// Snapshot-restored by `with_rebound_vars_scope` alongside
    /// `rebound_vars`.
    pub local_var_types: RefCell<HashMap<String, Ty>>,

    /// Names of locals that the Seq emit has rebound to their
    /// unwrapped shape via `let Some(x) = x else { ... };` (see
    /// `try_fuse_let_else` / `try_emit_param_guard_unwrap`).
    /// Subsequent Var reads of these names must NOT re-apply the
    /// narrowing-write-back `.clone().unwrap()` — the let-Some
    /// already produced an owned T.
    pub rebound_vars: RefCell<HashSet<String>>,

    /// Declared return type of the enclosing method, set by
    /// `method.rs` around each method-body emit. `None` outside a
    /// method body. `emit_expr` consults it for `return nil` lowering:
    /// when the method returns `Option<T>`, a bare Ruby `return nil`
    /// must emit `return None` rather than just `return` (E0069 in
    /// non-unit-returning functions).
    pub current_return_ty: RefCell<Option<Ty>>,

    /// True while rendering the body of a `pub fn new(...) -> Self`
    /// (Ruby `def initialize`). Rust constructors have no `self`
    /// mid-body — ivar reads emit as bare locals, ivar assigns emit
    /// as `let mut`. The caller appends `Self { f1, f2, ... }` at the
    /// end, building the instance from the locals. Set by
    /// `with_constructor_mode`.
    pub in_constructor: Cell<bool>,

    /// True while rendering the body of a `def self.X` (class method),
    /// emitted as `pub fn X(...)` with no `self` parameter. Ruby's
    /// `self` inside a class method *is* the class, so `SelfRef`
    /// emits as `Self`. Set by `with_class_method_scope`.
    pub in_class_method: Cell<bool>,

    /// True at expression positions whose value flows out of the
    /// enclosing function as the return value: top-level body emit,
    /// tail of a `Seq`, value of a `Return`. Reset when entering
    /// non-tail child positions (Send args, If conds, etc.). Lets
    /// the `Ivar` arm append `.clone()` for non-Copy fields read in
    /// tail position. Toggled by `with_return_tail`.
    pub in_return_tail: Cell<bool>,

    /// True while emitting a module-singleton class (Ruby pattern
    /// `module X; class << self; attr_accessor :slot; end; end`):
    /// all methods are class methods, "ivars" are module-level state
    /// stored in per-ivar `Mutex<Option<T>>` statics. Set by
    /// `with_module_singleton`.
    pub in_module_singleton: Cell<bool>,

    /// True while emitting a module singleton whose state is
    /// *request-scoped* (see `library.rs::REQUEST_SCOPED_SINGLETONS`):
    /// per-ivar slots are `thread_local!` `RefCell<Option<T>>` statics
    /// instead of global `Mutex<Option<T>>`. Only meaningful while
    /// `in_module_singleton` is set. Set by `with_module_singleton`.
    pub module_singleton_thread_local: Cell<bool>,

    /// True while emitting an expression that's the *immediate* recv
    /// of a method-call Send. At a recv position Rust's auto-ref
    /// (`(&v).method(...)` / `(&mut v).method(...)`) handles
    /// borrowing, so the decide-pass `CLONE_AT` bit's `.clone()`
    /// append is unnecessary AND breaks `&mut self` setters —
    /// `instance.clone().set_id(1)` mutates a discarded copy. Set
    /// transiently by `emit_send_recv` (only at Var-shaped recvs) so
    /// the Var arm can suppress its multi-read clone.
    pub suppress_var_clone: Cell<bool>,
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
