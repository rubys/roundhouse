//! `rust2` expression emit — `Expr` IR → Rust source-text.
//!
//! Phase 2.1 scope: minimal handling for the inflector body shape
//! (Lit, Var, Send `==`, StringInterp, If). Extended file-by-file
//! through Phase 2 as each runtime file forces new IR shapes.

use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};

mod assign;
mod literal;
mod send;
mod util;
use assign::emit_assign;
use literal::{attach_block, emit_array, emit_closure, emit_hash, emit_is_a, emit_string_interp};
pub(super) use literal::emit_literal;
use send::{cast_via_value_for_union, coerce_arg_for_field_ty, emit_send};
pub(super) use send::coerce_arg_for_param_ty;
pub(super) use util::{
    arm_body_already_value, coerce_to_value, emit_case_pattern, indent,
    is_builtin_container_class, is_copy_ty, is_option_of, is_option_ty,
    peel_nil, rewrite_method_name, sanitize_ident,
    synth_default_for_ty, ty_contains_untyped, value_narrowing_coercion,
};

thread_local! {
    /// True while rendering the body of a `pub fn new(...) -> Self`
    /// (Ruby `def initialize`). Rust constructors have no `self`
    /// mid-body — the ivar emit shifts:
    ///   `@x` (read) → bare `x` (local)
    ///   `@x = value` → `let mut x = value` (binds a local)
    /// The caller appends `Self { f1, f2, ... }` at the end, building
    /// the instance from the locals. `self.method(args)` calls now
    /// route through STATIC_METHODS (below) — methods marked static
    /// emit as `Self::method(args)` and compile pre-instance.
    static IN_CONSTRUCTOR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// True while rendering the body of a `def self.X` (class method),
    /// emitted as `pub fn X(...)` with no `self` parameter. Ruby's
    /// `self` inside a class method *is* the class, so `SelfRef` →
    /// `Self` and `SelfRef.method(args)` → `Self::method(args)`. The
    /// body-typer (see `analyze/body/mod.rs::resolves_through_self`)
    /// rewrites implicit-receiver class-method calls to explicit
    /// `recv = Some(SelfRef)`, so the emitter sees the explicit form
    /// regardless of how the Ruby source was written.
    static IN_CLASS_METHOD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// True at expression positions whose value flows out of the
    /// enclosing function as the return value: top-level body emit,
    /// tail of a `Seq`, value of a `Return`. Reset when entering
    /// non-tail child positions (Send args, If conds, etc.). Lets
    /// the `Ivar` arm append `.clone()` for non-Copy fields read in
    /// tail position — `pub fn body(&self) -> String { self.body }`
    /// otherwise moves out of `&self` (E0507). Off in constructor
    /// mode (the closing `Self { fields }` literal handles the
    /// return; ivars in the body are locals).
    static IN_RETURN_TAIL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };

    /// Methods in the current `impl` block that were classified as
    /// static-safe by `library.rs::method_reads_self`. When a Send
    /// targets one of these via implicit-`self` recv, emit as
    /// `Self::method(args)` rather than `self.method(args)` — the
    /// latter wouldn't compile inside `pub fn new` (no instance yet)
    /// and is also the cleaner Rust form for inherently-static
    /// helpers regardless of call-site context.
    static STATIC_METHODS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());

    /// Field names of the struct being constructed by the current
    /// `pub fn new`. Empty outside constructor scope. Lets `Return {
    /// Nil }` inside the constructor emit `return Self { f1, f2 }`
    /// instead of bare `return` — Ruby's `return if cond` early
    /// exit lowers to `Return { Nil }`, but the Rust constructor
    /// must produce `Self`.
    static CONSTRUCTOR_FIELDS: std::cell::RefCell<Vec<String>> =
        std::cell::RefCell::new(Vec::new());

    /// Variable names that the current method body assigns more
    /// than once. Pre-computed by `with_method_scope` and consulted
    /// by `emit_assign`. First-assignment site emits `let mut name =
    /// expr` (mutable binding); later sites emit plain `name = expr`
    /// (rebind, no shadow). Single-assignment locals stay
    /// immutable: `let name = expr`. Without this, Ruby `i = 0;
    /// while ...; i += 1; end` translated naively shadows `i`
    /// inside the loop and loops forever.
    ///
    /// Keyed on name (Symbol) rather than VarId — the body-typer's
    /// `VarId` is not unique per local in the runtime IR (locals
    /// within a method share `VarId(0)` until a true scope pass
    /// lands). Name-based tracking works because `with_method_scope`
    /// resets the set per method, so cross-method name collisions
    /// don't matter.
    static MUT_VARS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());
    /// Variable names the current method body has already emitted a
    /// `let` binding for. Subsequent `Assign LValue::Var` sites for
    /// the same name rebind without re-declaring.
    static DECLARED_VARS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());

    /// Variable names whose `local_var_ty` was set from the
    /// back-propagated function-return type (`empty_hash_return_ty`),
    /// not from the value's body-typer `Ty`. The Send `[]=` peephole
    /// uses this to know the recorded type is authoritative — for
    /// body-typer-derived `r.ty` the storage may disagree (e.g.
    /// `Hash<Sym, Str>` in IR but `HashMap<&str, String>` in emit).
    static BACK_PROPAGATED_HASH_LOCALS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());

    /// Method-name → positional-param-Ty map for the currently-
    /// emitting class. Populated by `library.rs` via
    /// `with_class_method_param_tys` before walking each class's
    /// methods. The Send walker for `Self::method(args)` consults
    /// this table to coerce args whose Hash<K, V> shape disagrees
    /// with the callee's declared param type — the
    /// `view_helpers::button_to → Self::render_attrs(form_attrs)`
    /// shape where `form_attrs` is locally `HashMap<&str, String>`
    /// but render_attrs declares `Hash[String, untyped]` =
    /// `HashMap<String, Value>`.
    static CLASS_METHOD_PARAM_TYS: std::cell::RefCell<
        std::collections::HashMap<String, Vec<crate::ty::Ty>>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());

    /// Variable names read more than once in the current method body.
    /// Populated by `with_method_scope`'s pre-pass via
    /// `collect_var_read_counts`. Consumed by the `Var` emit arm to
    /// append `.clone()` on every read when the type is non-Copy —
    /// over-clones the lexically-last read by one (cheap; the alternative
    /// is a final-use analysis the rust2 emit doesn't track). Closes
    /// the canonical "use after move" pattern: `let t = ...; if c1
    /// { f(t) }; if c2 { f(t) }` (no else dominates either way), and
    /// HashMap-literal entries that name the same Var in two values.
    static CLONE_VARS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());

    /// Ivar name → declared field type for the struct currently being
    /// emitted. Set by `library.rs` around each `impl` block so
    /// `emit_assign` can coerce mismatched RHS types (the canonical
    /// case is `self.body = ""`: literal `""` is `&str`, field is
    /// `String`; without coercion the Rust compiler rejects). Empty
    /// outside class-body scope; cleared between classes so stale
    /// entries don't bleed across emit units.
    static IVAR_TYPES: std::cell::RefCell<std::collections::HashMap<String, crate::ty::Ty>> =
        std::cell::RefCell::new(std::collections::HashMap::new());

    /// True while emitting a module-singleton class (Ruby pattern
    /// `module X; class << self; attr_accessor :slot; end; end`):
    /// all methods are class methods, "ivars" are module-level
    /// state. In this mode `@x` reads emit as
    /// `X_SLOT.lock().unwrap().clone()...` and `@x = v` emits as
    /// `*X_SLOT.lock().unwrap() = Some(v)`, against per-ivar
    /// `static` Mutex slots emitted alongside the impl block.
    static IN_MODULE_SINGLETON: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };

    /// Declared return type of the enclosing method, set by
    /// `method.rs` around each method-body emit. `None` outside a
    /// method body. `emit_expr` consults it for `return nil` lowering:
    /// when the method returns `Option<T>`, a bare Ruby `return nil`
    /// must emit `return None` rather than just `return` (which is
    /// E0069 in non-Unit-returning functions).
    static CURRENT_RETURN_TY: std::cell::RefCell<Option<crate::ty::Ty>> =
        std::cell::RefCell::new(None);

    /// Parameter name → declared RBS type for the enclosing method,
    /// set by `method.rs` around each method-body emit. The body-typer
    /// doesn't always propagate the param's Option-ness to Var reads,
    /// so `emit_assign`'s String coercion needs this side channel to
    /// avoid adding `.to_string()` to an `Option<String>`-typed param
    /// reference (which fails Display). Empty outside method body.
    static PARAM_TYPES: std::cell::RefCell<std::collections::HashMap<String, crate::ty::Ty>> =
        std::cell::RefCell::new(std::collections::HashMap::new());

    /// Names of locals that the Seq emit has rebound to their unwrapped
    /// shape via `let Some(x) = x else { ... };` (see
    /// `try_fuse_let_else` / `try_emit_param_guard_unwrap`). Subsequent
    /// Var reads of these names must NOT re-apply the narrowing-write-
    /// back `.clone().unwrap()` — the let-Some already produced an
    /// owned T. Without this scope, json_builder.rs::encode_datetime
    /// double-unwraps `s` and yields `s.clone().unwrap()` on a String.
    static REBOUND_VARS: std::cell::RefCell<std::collections::HashSet<String>> =
        std::cell::RefCell::new(std::collections::HashSet::new());

    /// Per-Seq tracking of local-var declared types. Populated by
    /// `Assign { LValue::Var, value }` sites with `value.ty` known.
    /// Read by the narrowing-aware Var emit so a local `params =
    /// match_pattern(...)` (Option<HashMap>) participates in the same
    /// narrowing+unwrap dance as an Option-typed function param —
    /// `unless params.nil?; ...; params; end` then emits a single
    /// `.clone().unwrap()` at the use site, matching the body-typer's
    /// narrowing. Snapshot-restored alongside REBOUND_VARS by the
    /// Seq emit's scope wrapper.
    static LOCAL_VAR_TYPES: std::cell::RefCell<std::collections::HashMap<String, crate::ty::Ty>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

pub(super) fn with_param_types<F, R>(types: std::collections::HashMap<String, crate::ty::Ty>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = PARAM_TYPES.with(|c| c.replace(types));
    let r = f();
    PARAM_TYPES.with(|c| *c.borrow_mut() = prev);
    r
}

pub(super) fn param_ty(name: &str) -> Option<crate::ty::Ty> {
    PARAM_TYPES.with(|c| c.borrow().get(name).cloned())
}

fn is_rebound_var(name: &str) -> bool {
    REBOUND_VARS.with(|c| c.borrow().contains(name))
}

fn mark_rebound_var(name: &str) {
    REBOUND_VARS.with(|c| {
        c.borrow_mut().insert(name.to_string());
    });
}

pub(super) fn local_var_ty(name: &str) -> Option<crate::ty::Ty> {
    LOCAL_VAR_TYPES.with(|c| c.borrow().get(name).cloned())
}

pub(super) fn mark_local_var_ty(name: &str, ty: crate::ty::Ty) {
    LOCAL_VAR_TYPES.with(|c| {
        c.borrow_mut().insert(name.to_string(), ty);
    });
}

/// Lookup a Var's declared type. Returns the function param's declared
/// Ty if present, else the most recent local assignment's RHS ty
/// recorded by the Seq emit. Used by the narrowing-aware Var read so
/// the same `.clone().unwrap()` Option-unwrap fires for `params =
/// match_pattern(...)` locals as for function params declared
/// `Option<T>` in RBS.
fn var_decl_ty(name: &str) -> Option<crate::ty::Ty> {
    param_ty(name).or_else(|| local_var_ty(name))
}

/// Snapshot the current REBOUND_VARS + LOCAL_VAR_TYPES, run `f`, then
/// restore the snapshot. Used by Seq emit to scope let-Some rebinds
/// and local declarations to the current Seq — nested blocks shouldn't
/// leak their bindings outward, and the surrounding emit shouldn't see
/// declarations from a child Seq.
fn with_rebound_vars_scope<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev_rebound = REBOUND_VARS.with(|c| c.borrow().clone());
    let prev_locals = LOCAL_VAR_TYPES.with(|c| c.borrow().clone());
    let r = f();
    REBOUND_VARS.with(|c| *c.borrow_mut() = prev_rebound);
    LOCAL_VAR_TYPES.with(|c| *c.borrow_mut() = prev_locals);
    r
}

pub(super) fn with_current_return_ty<F, R>(ty: Option<crate::ty::Ty>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = CURRENT_RETURN_TY.with(|c| c.replace(ty));
    let r = f();
    CURRENT_RETURN_TY.with(|c| *c.borrow_mut() = prev);
    r
}

pub(super) fn current_return_is_option() -> bool {
    CURRENT_RETURN_TY.with(|c| {
        matches!(
            c.borrow().as_ref(),
            Some(crate::ty::Ty::Union { variants }) if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
        )
    })
}

/// True when the enclosing function returns unit (`()` — declared
/// `-> void` in RBS, `Ty::Nil` in IR). The trailing `nil` of a void-
/// shaped Ruby method's body needs to emit as `()` (or nothing),
/// NOT as `None` (which is the Option::None constructor and would
/// produce an E0308 in a void function context).
pub(super) fn current_return_is_unit() -> bool {
    CURRENT_RETURN_TY.with(|c| matches!(c.borrow().as_ref(), Some(crate::ty::Ty::Nil)))
}

/// Run `f` with the module-singleton emit mode active. Used by
/// `library.rs` when the class shape signals a Ruby
/// `class << self; ... end` (every method is a class method).
pub(super) fn with_module_singleton<F, R>(active: bool, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = IN_MODULE_SINGLETON.with(|c| c.replace(active));
    let r = f();
    IN_MODULE_SINGLETON.with(|c| c.set(prev));
    r
}

pub(super) fn in_module_singleton() -> bool {
    IN_MODULE_SINGLETON.with(|c| c.get())
}

/// Slot identifier for an ivar in module-singleton emit. `@adapter`
/// → `ADAPTER`. Mirrors the SCREAMING_SNAKE Rust convention for
/// statics; the `_` stripping handles Ruby's leading-underscore
/// ivars (`@_foo` → `FOO`) and tail-underscore predicates aren't a
/// shape `attr_accessor` produces.
pub(super) fn module_singleton_slot_name(ivar: &str) -> String {
    ivar.trim_start_matches('_').to_uppercase()
}

/// Look up the declared field type for `name` within the struct
/// currently being emitted. `None` outside class-body scope or for
/// names not in the ivar table.
pub(super) fn ivar_field_ty(name: &str) -> Option<crate::ty::Ty> {
    IVAR_TYPES.with(|c| c.borrow().get(name).cloned())
}

/// Run `f` with the supplied ivar→type table active. Used by
/// `library.rs` to scope each `impl` block's emit.
pub(super) fn with_ivar_types<F, R>(types: std::collections::HashMap<String, crate::ty::Ty>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = IVAR_TYPES.with(|c| c.replace(types));
    let r = f();
    IVAR_TYPES.with(|c| *c.borrow_mut() = prev);
    r
}

pub(super) fn with_constructor_mode<F, R>(fields: Vec<String>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev_mode = IN_CONSTRUCTOR.with(|c| c.replace(true));
    let prev_fields = CONSTRUCTOR_FIELDS.with(|c| c.replace(fields));
    let r = f();
    IN_CONSTRUCTOR.with(|c| c.set(prev_mode));
    CONSTRUCTOR_FIELDS.with(|c| *c.borrow_mut() = prev_fields);
    r
}

/// Per-method emit scope: pre-walks `body` to identify multi-assign
/// VarIds (rendered with `let mut`), resets the declared-vars set,
/// and runs `f`. Used by `method.rs` around the body emit so each
/// method gets its own var-scope without leaking into the next.
pub(super) fn with_method_scope<F, R>(body: &Expr, f: F) -> R
where
    F: FnOnce() -> R,
{
    let mut counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    collect_var_assign_counts(body, &mut counts);
    let mut mut_vars: std::collections::HashSet<String> = counts
        .into_iter()
        .filter_map(|(name, n)| if n > 1 { Some(name) } else { None })
        .collect();
    // Vars used as the receiver of any Send call: the method may
    // take `&mut self` (e.g. `instance.save()` on a freshly-bound
    // `let instance = Self::new(...)`). Without `let mut`, the
    // borrow checker rejects with E0596. Conservative — flags every
    // method-receiver use as mut, even read-only ones. Rust emits a
    // benign `unused_mut` warning for those; the alternative would
    // require receiver-aware Ty inspection (whether `save` takes
    // `&mut self` vs `&self`) which the body-typer doesn't surface.
    collect_var_send_receivers(body, &mut mut_vars);
    // Pre-pass for `CLONE_VARS`: any local name read more than once
    // syntactically. Read-counts don't equal move-counts (literal
    // arguments, method-call receivers that take `&self`, narrowing-
    // rewritten reads via `.clone().unwrap()` etc. don't consume) —
    // but the over-clone is cheap and correct. The Var emit arm
    // gates the .clone() on `!is_copy_ty(e.ty)` so Int/Bool reads
    // stay unsuffixed.
    let mut read_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    collect_var_read_counts(body, &mut read_counts);
    let clone_vars: std::collections::HashSet<String> = read_counts
        .into_iter()
        .filter_map(|(name, n)| if n > 1 { Some(name) } else { None })
        .collect();
    let prev_mut = MUT_VARS.with(|c| c.replace(mut_vars));
    let prev_declared =
        DECLARED_VARS.with(|c| c.replace(std::collections::HashSet::new()));
    let prev_clone = CLONE_VARS.with(|c| c.replace(clone_vars));
    let prev_back_prop = BACK_PROPAGATED_HASH_LOCALS
        .with(|c| c.replace(std::collections::HashSet::new()));
    let r = f();
    MUT_VARS.with(|c| *c.borrow_mut() = prev_mut);
    DECLARED_VARS.with(|c| *c.borrow_mut() = prev_declared);
    CLONE_VARS.with(|c| *c.borrow_mut() = prev_clone);
    BACK_PROPAGATED_HASH_LOCALS.with(|c| *c.borrow_mut() = prev_back_prop);
    r
}

fn collect_var_send_receivers(
    e: &Expr,
    out: &mut std::collections::HashSet<String>,
) {
    match &*e.node {
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                if let ExprNode::Var { name, .. } = &*r.node {
                    out.insert(name.as_str().to_string());
                }
                collect_var_send_receivers(r, out);
            }
            args.iter().for_each(|a| collect_var_send_receivers(a, out));
            if let Some(b) = block { collect_var_send_receivers(b, out); }
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                collect_var_send_receivers(recv, out);
            }
            collect_var_send_receivers(value, out);
        }
        ExprNode::Seq { exprs } => exprs.iter().for_each(|e| collect_var_send_receivers(e, out)),
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_var_send_receivers(cond, out);
            collect_var_send_receivers(then_branch, out);
            collect_var_send_receivers(else_branch, out);
        }
        ExprNode::While { cond, body, .. } => {
            collect_var_send_receivers(cond, out);
            collect_var_send_receivers(body, out);
        }
        ExprNode::Return { value } => collect_var_send_receivers(value, out),
        ExprNode::Hash { entries, .. } => entries.iter().for_each(|(k, v)| {
            collect_var_send_receivers(k, out);
            collect_var_send_receivers(v, out);
        }),
        ExprNode::Array { elements, .. } => {
            elements.iter().for_each(|e| collect_var_send_receivers(e, out))
        }
        ExprNode::StringInterp { parts } => parts.iter().for_each(|p| {
            if let InterpPart::Expr { expr } = p {
                collect_var_send_receivers(expr, out);
            }
        }),
        ExprNode::BoolOp { left, right, .. } => {
            collect_var_send_receivers(left, out);
            collect_var_send_receivers(right, out);
        }
        ExprNode::Lambda { body, .. } => collect_var_send_receivers(body, out),
        _ => {}
    }
}

/// Count `ExprNode::Var` reads per name across the method body.
/// Mirrors `collect_var_assign_counts`'s recursive walk shape but
/// counts reads instead of assignments. Names with count > 1 land
/// in `CLONE_VARS` so the `Var` emit arm appends `.clone()` for
/// non-Copy types — the use-after-move guard that closes
/// `view_helpers::submit`'s HashMap-literal-repeats and
/// `active_record::Base::fill_timestamps`'s `now` across two ifs.
fn collect_var_read_counts(
    e: &Expr,
    out: &mut std::collections::HashMap<String, usize>,
) {
    match &*e.node {
        ExprNode::Var { name, .. } => {
            *out.entry(name.as_str().to_string()).or_insert(0) += 1;
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                collect_var_read_counts(recv, out);
            }
            collect_var_read_counts(value, out);
        }
        ExprNode::Seq { exprs } => exprs.iter().for_each(|e| collect_var_read_counts(e, out)),
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_var_read_counts(cond, out);
            collect_var_read_counts(then_branch, out);
            collect_var_read_counts(else_branch, out);
        }
        ExprNode::While { cond, body, .. } => {
            collect_var_read_counts(cond, out);
            collect_var_read_counts(body, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv { collect_var_read_counts(r, out); }
            args.iter().for_each(|a| collect_var_read_counts(a, out));
            if let Some(b) = block { collect_var_read_counts(b, out); }
        }
        ExprNode::Return { value } => collect_var_read_counts(value, out),
        ExprNode::Hash { entries, .. } => entries.iter().for_each(|(k, v)| {
            collect_var_read_counts(k, out);
            collect_var_read_counts(v, out);
        }),
        ExprNode::Array { elements, .. } => {
            elements.iter().for_each(|e| collect_var_read_counts(e, out))
        }
        ExprNode::StringInterp { parts } => parts.iter().for_each(|p| {
            if let InterpPart::Expr { expr } = p {
                collect_var_read_counts(expr, out);
            }
        }),
        ExprNode::BoolOp { left, right, .. } => {
            collect_var_read_counts(left, out);
            collect_var_read_counts(right, out);
        }
        ExprNode::Lambda { body, .. } => collect_var_read_counts(body, out),
        _ => {}
    }
}

fn collect_var_assign_counts(
    e: &Expr,
    out: &mut std::collections::HashMap<String, usize>,
) {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            *out.entry(name.as_str().to_string()).or_insert(0) += 1;
            collect_var_assign_counts(value, out);
        }
        ExprNode::Assign { target, value } => {
            if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                collect_var_assign_counts(recv, out);
            }
            collect_var_assign_counts(value, out);
        }
        ExprNode::Seq { exprs } => exprs.iter().for_each(|e| collect_var_assign_counts(e, out)),
        ExprNode::If { cond, then_branch, else_branch } => {
            collect_var_assign_counts(cond, out);
            collect_var_assign_counts(then_branch, out);
            collect_var_assign_counts(else_branch, out);
        }
        ExprNode::While { cond, body, .. } => {
            collect_var_assign_counts(cond, out);
            collect_var_assign_counts(body, out);
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv { collect_var_assign_counts(r, out); }
            args.iter().for_each(|a| collect_var_assign_counts(a, out));
            if let Some(b) = block { collect_var_assign_counts(b, out); }
        }
        ExprNode::Return { value } => collect_var_assign_counts(value, out),
        ExprNode::Hash { entries, .. } => entries
            .iter()
            .for_each(|(k, v)| {
                collect_var_assign_counts(k, out);
                collect_var_assign_counts(v, out);
            }),
        ExprNode::Array { elements, .. } => {
            elements.iter().for_each(|e| collect_var_assign_counts(e, out))
        }
        ExprNode::StringInterp { parts } => parts.iter().for_each(|p| {
            if let InterpPart::Expr { expr } = p {
                collect_var_assign_counts(expr, out);
            }
        }),
        _ => {}
    }
}

fn render_self_literal() -> String {
    CONSTRUCTOR_FIELDS.with(|c| {
        let fields = c.borrow();
        if fields.is_empty() {
            "Self {}".to_string()
        } else {
            format!("Self {{ {} }}", fields.join(", "))
        }
    })
}

/// Run `f` with `methods` registered as the current class's static-
/// method set. Used by `library.rs::emit_library_class` to scope the
/// static-method dispatch decision to the impl block being rendered.
pub(super) fn with_static_methods<F, R>(
    methods: std::collections::HashSet<String>,
    f: F,
) -> R
where
    F: FnOnce() -> R,
{
    let prev = STATIC_METHODS.with(|c| c.replace(methods));
    let r = f();
    STATIC_METHODS.with(|c| *c.borrow_mut() = prev);
    r
}

/// Set the current class's method-name → positional-param-Tys
/// table for the duration of `f`. Used by `library.rs` to seed the
/// `Self::method(args)` arg-coercion lookup in emit_send.
pub(super) fn with_class_method_param_tys<F, R>(
    map: std::collections::HashMap<String, Vec<crate::ty::Ty>>,
    f: F,
) -> R
where
    F: FnOnce() -> R,
{
    let prev = CLASS_METHOD_PARAM_TYS.with(|c| c.replace(map));
    let r = f();
    CLASS_METHOD_PARAM_TYS.with(|c| *c.borrow_mut() = prev);
    r
}

/// Look up the current class's method param types by method name.
/// Returns None outside any class scope or when the method isn't
/// in the current class's table.
pub(super) fn class_method_param_ty(method: &str, idx: usize) -> Option<crate::ty::Ty> {
    CLASS_METHOD_PARAM_TYS
        .with(|c| c.borrow().get(method).and_then(|tys| tys.get(idx).cloned()))
}

/// Return the full Vec of positional param Tys for a method in the
/// current class. Used by the Const-recv dispatch to check arity
/// + pad missing trailing args with defaults — Ruby's `def
/// initialize(attrs = {})` accepts zero-arg `Article.new`, but
/// Rust requires the explicit `HashMap::new()` default.
pub(super) fn current_class_method_param_tys(method: &str) -> Option<Vec<crate::ty::Ty>> {
    CLASS_METHOD_PARAM_TYS
        .with(|c| c.borrow().get(method).cloned())
}

fn in_constructor() -> bool {
    IN_CONSTRUCTOR.with(|c| c.get())
}

fn in_class_method() -> bool {
    IN_CLASS_METHOD.with(|c| c.get())
}

pub(super) fn with_class_method_scope<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = IN_CLASS_METHOD.with(|c| c.replace(true));
    let r = f();
    IN_CLASS_METHOD.with(|c| c.set(prev));
    r
}

pub(super) fn current_return_ty() -> Option<crate::ty::Ty> {
    CURRENT_RETURN_TY.with(|c| c.borrow().clone())
}

pub(super) fn is_declared_var(name: &str) -> bool {
    DECLARED_VARS.with(|c| c.borrow().contains(name))
}

pub(super) fn declare_var(name: String) {
    DECLARED_VARS.with(|c| {
        c.borrow_mut().insert(name);
    });
}

pub(super) fn is_mut_var(name: &str) -> bool {
    MUT_VARS.with(|c| c.borrow().contains(name))
}

pub(super) fn record_back_propagated_hash(name: String) {
    BACK_PROPAGATED_HASH_LOCALS.with(|c| {
        c.borrow_mut().insert(name);
    });
}

pub(super) fn is_back_propagated_hash(name: &str) -> bool {
    BACK_PROPAGATED_HASH_LOCALS.with(|c| c.borrow().contains(name))
}

pub(super) fn in_return_tail() -> bool {
    IN_RETURN_TAIL.with(|c| c.get())
}

/// Set the return-tail flag and run `f`. Used by `method.rs` around
/// the body emit of non-constructor instance methods, so the body's
/// top-level expression (or `Seq` tail / `Return` value) is recognized
/// as the function's return value.
pub(super) fn with_return_tail<F, R>(value: bool, f: F) -> R
where
    F: FnOnce() -> R,
{
    let prev = IN_RETURN_TAIL.with(|c| c.replace(value));
    let r = f();
    IN_RETURN_TAIL.with(|c| c.set(prev));
    r
}

pub(super) fn is_static_method(name: &str) -> bool {
    STATIC_METHODS.with(|c| c.borrow().contains(name))
}

pub(super) fn emit_expr(e: &Expr) -> String {
    // Public entry clears IN_RETURN_TAIL: any caller recursing into a
    // child expression is, by default, not in return tail. The Seq /
    // Return / If arms re-enable the flag for their tail children via
    // `emit_expr_tail`.
    let raw = with_return_tail(false, || emit_expr_inner(e));
    apply_str_coercion(raw, e)
}

/// Tail-preserving emit. Caller is responsible for ensuring this is
/// invoked only at tail positions of the enclosing function (e.g.,
/// `Seq`'s last expression, `Return`'s value, `If`'s branches when
/// the `If` itself is in tail position).
pub(super) fn emit_expr_tail(e: &Expr) -> String {
    apply_str_coercion(emit_expr_inner(e), e)
}

/// Wrap `raw` with the str-coercion shape recorded by
/// `analyze::str_color`. Single application point so per-node match
/// arms in `emit_expr_inner` can keep producing the natural
/// non-coerced shape; coercions land here based on `e.str_coercion`
/// once and don't have to be re-derived per node kind.
///
/// Defensive parens around the inner emit keep the surrounding
/// expression context safe — `&` and `.to_string()` both have
/// surprising precedence when the inner is a method-call chain or
/// arithmetic expression.
fn apply_str_coercion(raw: String, e: &Expr) -> String {
    match e.str_coercion {
        None => raw,
        Some(crate::expr::StrCoercion::Borrow) => format!("&({raw})"),
        Some(crate::expr::StrCoercion::ToOwned) => format!("({raw}).to_string()"),
    }
}

fn emit_expr_inner(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => {
            // Narrowing write-back: when the body-typer narrows
            // `content_type` (Option<String>) to `String` inside an
            // `unless content_type.nil?` body, e.ty reflects the
            // narrowed type but the Rust binding is still
            // `Option<String>` (per the function signature). Insert
            // `.clone().unwrap()` so the rendered RHS matches what
            // downstream coercion paths see in `e.ty`. Only fires when
            // the param table declares Option-shape AND the narrowed
            // e.ty is the unwrapped variant — the common nil-narrowing
            // pattern. The `.clone()` keeps the binding usable on
            // multiple reads in the same scope.
            let n = name.as_str();
            if let Some(narrowed) = &e.ty {
                if let Some(declared) = var_decl_ty(n) {
                    if is_option_of(&declared, narrowed) && !is_rebound_var(n) {
                        return format!("{n}.clone().unwrap()");
                    }
                    // Value narrowing via `is_a?(Class)`: a param typed
                    // Untyped (→ `serde_json::Value`) narrowed inside an
                    // `if v.is_a?(String)` branch needs `.as_str().unwrap()`
                    // shape so the value participates in str-typed call
                    // sites. Without this, `encode_string(v)` in
                    // json_builder.rs gets `Value` where `&str` is expected.
                    if matches!(declared, crate::ty::Ty::Untyped) {
                        if let Some(coerce) = value_narrowing_coercion(narrowed) {
                            return format!("{n}.{coerce}");
                        }
                    }
                }
            }
            // Multi-read non-Copy local: clone on every read so a
            // later use-after-the-first doesn't trip E0382. The pre-
            // pass in `with_method_scope` records names read > 1
            // times; the type-gate keeps Int/Bool/Float reads
            // suffix-free. Over-clones the lexically-last read by
            // one, fine until a final-use analysis lands.
            let needs_clone = CLONE_VARS.with(|c| c.borrow().contains(n));
            let is_non_copy = e.ty.as_ref().map(|t| !is_copy_ty(t)).unwrap_or(false);
            if needs_clone && is_non_copy {
                return format!("{n}.clone()");
            }
            n.to_string()
        }
        ExprNode::Ivar { name } => {
            if in_module_singleton() {
                // Module-singleton ivar read — pull through the
                // Mutex<Option<T>> slot emitted alongside the impl.
                // `clone().unwrap_or_default()` matches Ruby's "nil
                // until set" semantics: every read after a `set_X`
                // sees the latest value, reads before init return a
                // default (the field type's `Default::default()`).
                // Callers expect a non-Option return type per RBS;
                // `Option<T>` ivars stay None-able via the inner T.
                let slot = module_singleton_slot_name(name.as_str());
                return format!(
                    "{slot}.lock().unwrap().clone().unwrap_or_default()"
                );
            }
            if in_constructor() {
                name.as_str().to_string()
            } else if in_return_tail()
                && matches!(e.ty.as_ref(), Some(t) if !is_copy_ty(t))
            {
                // Tail-position read of a non-Copy field would move
                // out of `&self`. `attr_reader`-shaped getters are the
                // canonical case (`def body; @body; end`); also kicks
                // in for any tail-`@x` body.
                format!("self.{name}.clone()")
            } else {
                format!("self.{name}")
            }
        }
        ExprNode::SelfRef => {
            if in_class_method() { "Self".to_string() } else { "self".to_string() }
        }
        ExprNode::Const { path } => {
            // Rust uses file-as-module — `ActiveSupport::HashWithIndifferentAccess`
            // in source becomes `crate::hash_with_indifferent_access::
            // HashWithIndifferentAccess` at import time, while in-file
            // self-references use the bare type name. Strip the
            // namespace and emit the last segment; cross-file refs
            // surface as missing imports in later phases (Phase 3+
            // when the module-tree resolver lands).
            path.last().map(|s| s.to_string()).unwrap_or_default()
        }
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::If { cond, then_branch, else_branch } => {
            // Ruby `cond ? a : b` and `if cond; a; else b; end` both
            // lower to `ExprNode::If`. The lowerer also produces this
            // shape for the modifier forms `STMT if COND` / `STMT
            // unless COND`, with the absent else branch synthesized
            // as `Nil`. Two cases trigger statement-form `if cond {
            // ... }` (no else clause):
            //   1. then diverges (Return/Raise) AND else is Nil —
            //      `return X if cond`. The else is dead code after
            //      the diverging then.
            //   2. else is Nil (period) — `errors << "msg" if cond`
            //      style. The implicit else=nil in Ruby returns nil
            //      from the conditional; in Rust the statement form
            //      returns `()` from the conditional, which matches
            //      `Option<...>::None` and statement-position uses
            //      well enough that it's the right default.
            // Both-branches-present cases (ternary, `if/else/end`
            // with non-Nil else) keep the expression form.
            let else_is_nil = matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            let then_is_nil = matches!(
                &*then_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            // Branches inherit the enclosing function's return-tail
            // flag: an `if/else` in tail position has both branches in
            // tail position; the cond is not.
            if else_is_nil {
                // In the tail position of an `Option<T>`-returning
                // function, emit `if X { Some(Y) } else { None }` so
                // the if-expression's type matches the function
                // return. Otherwise emit the statement-form `if X { Y }`
                // (returns `()`, OK for void statement context).
                //
                // `Some({ then_s })` instead of `Some(then_s)` so a
                // multi-statement Seq branch (the common case for
                // lowerer-inlined adapter-find bodies — `let stmt =
                // …; let mut result = None; … result`) parses: `Some`
                // takes an expression and a bare statement list isn't
                // one, but a `{ … }` block evaluating to its tail
                // expression is. Adds harmless redundant braces for
                // single-expression branches.
                let cond_s = emit_expr(cond);
                let then_s = with_declared_vars_scope(|| emit_expr_tail(then_branch));
                if in_return_tail() && current_return_is_option() {
                    // Skip the Some wrap when the inner branch already
                    // produces an `Option<T>` — Comment#article's
                    // adapter-find body ends in `result` where rust2
                    // typed the let-binding `Option<Article>` via
                    // `none_init_option_return_ty` back-prop. Wrapping
                    // again would produce `Option<Option<T>>`.
                    if tail_produces_option(then_branch) {
                        return format!("if {cond_s} {{ {{ {then_s} }} }} else {{ None }}");
                    }
                    return format!("if {cond_s} {{ Some({{ {then_s} }}) }} else {{ None }}");
                }
                return format!("if {cond_s} {{ {then_s} }}");
            }
            // `STMT unless COND` lowers to `If { cond, then: Nil, else:
            // STMT }` — emit as the negated single-branch form so the
            // Nil-vs-Assign branch mismatch (E0308 "if and else have
            // incompatible types") doesn't surface. Symmetric with
            // the else_is_nil case above.
            if then_is_nil {
                let cond_s = emit_expr(cond);
                let else_s = with_declared_vars_scope(|| emit_expr_tail(else_branch));
                if in_return_tail() && current_return_is_option() {
                    if tail_produces_option(else_branch) {
                        return format!(
                            "if !({cond_s}) {{ {{ {else_s} }} }} else {{ None }}"
                        );
                    }
                    return format!(
                        "if !({cond_s}) {{ Some({{ {else_s} }}) }} else {{ None }}"
                    );
                }
                return format!(
                    "if !({cond_s}) {{ {else_s} }}"
                );
            }
            // Per-branch DECLARED_VARS scope: each branch's body is a
            // separate Rust scope, so a `let json = X` in one branch
            // doesn't carry the binding into the other branch or the
            // statements after the if. Snapshot/restore around each
            // branch emit so a subsequent `json = Y` re-emits as
            // `let json = Y` (first-use-in-the-new-scope) rather than
            // a bare `json = Y` that fails E0425. Mirrors how Rust
            // scoping actually works.
            let cond_s = emit_expr(cond);
            let then_s = with_declared_vars_scope(|| emit_expr_tail(then_branch));
            let else_s = with_declared_vars_scope(|| emit_expr_tail(else_branch));
            format!("if {cond_s} {{ {then_s} }} else {{ {else_s} }}")
        }
        ExprNode::Send { recv, method, args, block, .. } => {
            // `recv.each { ... }` on Hash / Vec — Ruby returns the
            // receiver after iterating; Rust has no `each` method on
            // these types. Emit as `.iter().for_each(...)` (Hash) /
            // `.iter_mut().for_each(...)` (Vec) so the closure
            // attaches against a stdlib method that accepts an
            // FnMut. For Hash, the closure params reshape into a
            // tuple destructure `|(k, v)|` to match `iter()`'s pair
            // yield. Recv-type-aware: only fires on the explicit
            // Vec/Hash receivers; untyped (serde_json::Value)
            // receivers fall through to the generic path (their
            // `.each` shape needs a per-value-shape bridge that's
            // separate work).
            if method.as_str() == "each" && args.is_empty() && recv.is_some() {
                let r = recv.as_ref().unwrap();
                let block_lambda: Option<(&[crate::ident::Symbol], &Expr)> =
                    block.as_ref().and_then(|b| match &*b.node {
                        ExprNode::Lambda { params, body, .. } => {
                            Some((params.as_slice(), body))
                        }
                        _ => None,
                    });
                if let Some((params, body)) = block_lambda {
                    if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. })) && params.len() == 2 {
                        let recv_s = emit_expr(r);
                        let k = params[0].as_str();
                        let v = params[1].as_str();
                        let body_s = emit_expr(body);
                        let closure = if body_s.contains('\n') {
                            format!("|({k}, {v})| {{\n{}\n}}", indent(&body_s, 1))
                        } else {
                            // Trailing `;` on the body so the closure
                            // produces `()`. `.for_each` requires
                            // `FnMut(&T) -> ()`; without the `;` the
                            // body's tail expression value becomes
                            // the closure return, which fails the
                            // unit-return signature on e.g.
                            // `records.each { |r| r.destroy }`.
                            format!("|({k}, {v})| {{ {body_s}; }}")
                        };
                        // `Hash<Untyped, Untyped>` is the post-narrowing
                        // shape `is_a?(Hash)` produces for a Value-typed
                        // var (analyze/body/narrowing.rs:122). Runtime
                        // storage stays `serde_json::Value`, which has
                        // no `.iter()` — route through `.as_object()`
                        // for a `serde_json::Map<String, Value>` whose
                        // `.iter()` yields `(&String, &Value)`.
                        let value_shaped = matches!(
                            r.ty.as_ref(),
                            Some(crate::ty::Ty::Hash { key, value })
                                if matches!(**key, crate::ty::Ty::Untyped)
                                    && matches!(**value, crate::ty::Ty::Untyped)
                        );
                        if value_shaped {
                            return format!(
                                "{recv_s}.as_object().unwrap().iter().for_each({closure})"
                            );
                        }
                        return format!("{recv_s}.iter().for_each({closure})");
                    }
                    let is_array_after_peel = matches!(
                        r.ty.as_ref().map(peel_nil),
                        Some(crate::ty::Ty::Array { .. })
                    );
                    if is_array_after_peel && params.len() == 1 {
                        let recv_s = emit_expr(r);
                        let p = params[0].as_str();
                        let body_s = emit_expr(body);
                        let closure = if body_s.contains('\n') {
                            format!("|{p}| {{\n{};\n}}", indent(&body_s, 1))
                        } else {
                            format!("|{p}| {{ {body_s}; }}")
                        };
                        // `Option<Vec<T>>` recv (`Union<Nil, Array>`):
                        // `.iter().flatten().for_each(...)` so the
                        // closure receives `&T` from the inner Vec
                        // rather than `Vec<T>` from Option's iter (one
                        // item if Some). Read-only `iter()` because
                        // mutating-through-Option needs an as_mut +
                        // unwrap chain that's overkill for the read-
                        // only `parts << ...` framework Ruby uses.
                        let was_option = matches!(
                            r.ty.as_ref(),
                            Some(crate::ty::Ty::Union { variants })
                                if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                        );
                        let iter_chain = if was_option {
                            ".iter().flatten()"
                        } else {
                            ".iter_mut()"
                        };
                        return format!("{recv_s}{iter_chain}.for_each({closure})");
                    }
                }
            }
            // `vec.map { |x| ... }` — Ruby returns a new Array of the
            // block's return value. Rust Vec has no `.map`; emit as
            // `.into_iter().map(...).collect::<Vec<_>>()`. The block's
            // body becomes a closure passed to Iterator::map.
            //
            // `into_iter` (not `iter`) so the closure receives the
            // element by value — matches Ruby's pass-by-value yield
            // and avoids forcing the block to `.clone()` everything
            // it reads from `x`. The receiver's owned-vs-borrowed
            // nature determines whether `into_iter` consumes; for
            // function-return Vec receivers (the common case here,
            // `adapter.all(...).map { ... }`) the temporary is moved
            // anyway.
            if method.as_str() == "map" && args.is_empty() && recv.is_some() {
                let r = recv.as_ref().unwrap();
                if matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Array { .. })) {
                    let block_lambda: Option<(&[crate::ident::Symbol], &Expr)> =
                        block.as_ref().and_then(|b| match &*b.node {
                            ExprNode::Lambda { params, body, .. } => {
                                Some((params.as_slice(), body))
                            }
                            _ => None,
                        });
                    if let Some((params, body)) = block_lambda {
                        if params.len() == 1 {
                            let recv_s = emit_expr(r);
                            let p = params[0].as_str();
                            let body_s = emit_expr(body);
                            let closure = if body_s.contains('\n') {
                                format!("|{p}| {{\n{}\n}}", indent(&body_s, 1))
                            } else {
                                format!("|{p}| {{ {body_s} }}")
                            };
                            // `Option<Vec<T>>` recv — `.iter().flatten()`
                            // borrows the Option, yields `&Vec<T>` then
                            // `&T`. `iter` (not `into_iter`) so a follow-
                            // up `recv.each` against the same Option
                            // (the `javascript_importmap_tags` shape:
                            // `pins.map { ... }; pins.each { ... }`)
                            // doesn't trip a borrow-after-move. The
                            // closure receives `&T`; Display/Index on
                            // `&Value` matches Ruby's by-value yield.
                            let was_option = matches!(
                                r.ty.as_ref(),
                                Some(crate::ty::Ty::Union { variants })
                                    if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                            );
                            let iter_chain = if was_option {
                                ".iter().flatten()"
                            } else {
                                ".into_iter()"
                            };
                            return format!(
                                "{recv_s}{iter_chain}.map({closure}).collect::<Vec<_>>()"
                            );
                        }
                    }
                }
            }
            let base = emit_send(recv.as_ref(), method.as_str(), args);
            // A Send with attached block becomes a closure passed as
            // the last arg. `other.each do |k, v| ... end` (Ruby) →
            // `other.each(|k, v| { ... })` (Rust). Whether the
            // receiver-type's method actually accepts a closure is
            // a per-target concern; the emit shape is right and the
            // type-checker surfaces mismatches when present.
            match block.as_ref() {
                None => base,
                Some(b) => attach_block(&base, b),
            }
        }
        ExprNode::Lambda { params, block_param: _, body, .. } => {
            // Standalone lambda (e.g. `-> { ... }` or `lambda { |x| x }`)
            // emits as a Rust closure literal. Block params are
            // re-emitted as bare names; type inference at the call
            // site fills in the rest. Multi-line bodies wrap in `{}`.
            emit_closure(params, body)
        }
        ExprNode::Yield { args } => {
            // `yield x, y` in Ruby calls the implicit block param.
            // rust2 represents this as a call to a closure-typed
            // parameter named `f` injected by the signature pass
            // (next commit). Until that pass lands, the call site
            // emits but won't compile — the body shape is right.
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            format!("f({})", args_s.join(", "))
        }
        ExprNode::Seq { exprs } => with_rebound_vars_scope(|| {
            // Rust statements are `;`-terminated; the last expression
            // is the block's value (no trailing `;`). Multi-statement
            // method bodies render natural Rust shape this way. The
            // tail expression inherits the enclosing function's
            // return-tail flag (`emit_expr_tail`) so e.g. a bare
            // `@field` at the end of a getter body becomes
            // `self.field.clone()`.
            let mut lines = Vec::with_capacity(exprs.len());
            let last = exprs.len().saturating_sub(1);
            let mut i = 0;
            while i < exprs.len() {
                // Guard-clause let-else fusion: detect
                //   let x = OPT;
                //   if x.nil? { return nil };  (or raise, etc.)
                //   ... uses of x narrowed to non-nil ...
                // and emit as
                //   let Some(x) = OPT else { return None };
                // The body-typer narrows `x` to non-nil for the
                // subsequent statements (see body/mod.rs Seq's
                // diverging-then narrowing), but `let mut x = OPT`
                // in Rust source still types as `Option<T>` and
                // subsequent reads fail E0308. The let-else form
                // hands Rust the same narrowing the body-typer has
                // already proven, no `.unwrap()` or rebind required.
                if i + 1 <= last {
                    if let Some((name, rendered)) = try_fuse_let_else(&exprs[i], &exprs[i + 1]) {
                        mark_rebound_var(&name);
                        lines.push(format!("{rendered};"));
                        i += 2;
                        continue;
                    }
                }
                // Standalone guard-clause unwrap: a Seq stmt of the
                // form `if x.nil? { return Y }` (or raise) where `x`
                // is a Var. The body-typer narrows `x` to non-nil for
                // subsequent statements, but in Rust source `x` is
                // still `Option<T>` — subsequent `x.method()` calls
                // fail. Rewrite to `let Some(x) = x else { return Y; };`
                // — rebinds `x` to the unwrapped value, matching the
                // body-typer's narrowing.
                if let Some((name, rendered)) = try_emit_param_guard_unwrap(&exprs[i]) {
                    mark_rebound_var(&name);
                    lines.push(format!("{rendered};"));
                    i += 1;
                    continue;
                }
                let e = &exprs[i];
                // Trailing `nil` in a void-return Ruby method
                // (`@x = y; nil` shape) — Lit::Nil emits as `None`
                // (Option::None constructor), which fails E0308 in a
                // function declared `-> ()`. Drop the trailing Nil
                // entirely; Rust functions implicitly return `()` at
                // the end of a block.
                if i == last
                    && current_return_is_unit()
                    && matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
                {
                    if !lines.is_empty() {
                        let last_line = lines.last_mut().unwrap();
                        if !last_line.ends_with(';') {
                            last_line.push(';');
                        }
                    }
                    i += 1;
                    continue;
                }
                let s = if i == last {
                    emit_expr_tail(e)
                } else {
                    emit_expr(e)
                };
                if i == last {
                    lines.push(s);
                } else {
                    lines.push(format!("{s};"));
                }
                i += 1;
            }
            lines.join("\n")
        }),
        ExprNode::Assign { target, value } => emit_assign(target, value),
        ExprNode::Return { value } => {
            let is_nil = matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
            // Constructor early returns produce `Self { fields }` —
            // Ruby's `return if cond` lowers to `Return { Nil }`, but
            // a `pub fn new(...) -> Self` body returning bare `()`
            // wouldn't typecheck. Explicit `return <expr>` keeps its
            // value (callers wanting different early-return values
            // can still write `return Self::new(...)` etc).
            if in_constructor() && is_nil {
                return format!("return {}", render_self_literal());
            }
            if is_nil {
                // `return nil` in a method declared `-> T?` (lowered
                // as `Option<T>`) must emit `return None`; bare
                // `return` is E0069 outside `() / Unit` returns. Plain
                // `return` is still correct for `void` Ruby methods
                // (RBS `-> void` lowers to `Ty::Nil` → Rust `()`).
                if current_return_is_option() {
                    "return None".to_string()
                } else {
                    "return".to_string()
                }
            } else {
                // String-literal return value in a String-returning
                // function: append `.to_string()`. The literal emits as
                // `&'static str` but the function signature is `String`.
                // This handles `return "" if X.nil?` patterns in
                // encode_string / encode_datetime where the early-exit
                // string needs to match the return type.
                //
                // Skip when `analyze::str_color` already annotated the
                // value — the new pass owns return-value ownership
                // coloring; double-applying the peephole would yield
                // `(value).to_string().to_string()`.
                let str_color_handled = value.str_coercion.is_some();
                let needs_to_string = !str_color_handled
                    && matches!(
                        &*value.node,
                        ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                    )
                    && CURRENT_RETURN_TY.with(|c| {
                        matches!(
                            c.borrow().as_ref(),
                            Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym)
                        )
                    });
                // `return self` in a method declared `-> Base` (owned
                // return). `self` is `&self` / `&mut self`; bare emit
                // produces `return self` typed as `&Base` /
                // `&mut Base`. Clone to satisfy the owned return type.
                let needs_self_clone = matches!(&*value.node, ExprNode::SelfRef)
                    && CURRENT_RETURN_TY.with(|c| {
                        matches!(c.borrow().as_ref(), Some(crate::ty::Ty::Class { .. }))
                    });
                // `return X` in an Option<T>-returning fn where X is
                // typed T (non-Option). The body-typer never inserts
                // Some-wrap; it's emit's job. Without this, an
                // `unless cond; return MatchResult.new(...); end` in
                // router.rb fails with "expected Option<MatchResult>,
                // found MatchResult".
                let needs_some_wrap = current_return_is_option()
                    && match value.ty.as_ref() {
                        Some(t) if !is_option_ty(t) => true,
                        None => false,
                        _ => false,
                    };
                if needs_to_string {
                    format!("return {}.to_string()", emit_expr_tail(value))
                } else if needs_self_clone {
                    "return self.clone()".to_string()
                } else if needs_some_wrap {
                    format!("return Some({})", emit_expr_tail(value))
                } else {
                    format!("return {}", emit_expr_tail(value))
                }
            }
        }
        ExprNode::While { cond, body, until_form } => {
            // Rust has no `until`; rewrite to `while !cond` for parity.
            let cond_s = emit_expr(cond);
            let body_s = emit_expr(body);
            let cond_clause = if *until_form {
                format!("!({cond_s})")
            } else {
                cond_s
            };
            format!("while {cond_clause} {{\n{}\n}}", indent(&body_s, 1))
        }
        ExprNode::Hash { entries, .. } => emit_hash(entries),
        ExprNode::Array { elements, .. } => emit_array(elements),
        ExprNode::Range { begin, end, exclusive } => {
            // Ruby `..` is inclusive end; Rust `..=` is inclusive end.
            // Ruby `...` is exclusive end; Rust `..` is exclusive end.
            // Mapping swaps the operator-shape: Ruby inclusive uses
            // two dots, Rust inclusive uses two-dots-equals.
            let op = if *exclusive { ".." } else { "..=" };
            let b = begin.as_ref().map(emit_expr).unwrap_or_default();
            let e = end.as_ref().map(emit_expr).unwrap_or_default();
            // Endless ranges (`1..`, `..5`) — Ruby inclusive endless
            // is `1..` (no end). Rust `1..` is also endless but
            // exclusive-shaped; the `..=` form requires a right
            // operand, so endless-inclusive collapses to plain `..`
            // unconditionally. Slice indexing (`pp[1..]`) is the
            // common case; semantics match either way for "from i
            // to end."
            if end.is_none() {
                return format!("{b}..");
            }
            if begin.is_none() {
                return format!("..{e}");
            }
            format!("{b}{op}{e}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            // Ruby `a && b` / `a || b` are truthy-on-non-nil-non-false,
            // not bool-typed. Rust's `||` / `&&` are bool-only — direct
            // emit only works when both operands are already Ty::Bool.
            //
            // For `Or` with a non-bool LHS, the idiomatic Ruby use is
            // "default value if LHS is nil/missing": `a || b` →
            //   - LHS Option<T>: `a.unwrap_or(b)`
            //   - LHS non-Option (String/Int/Class instance): `a`
            //     alone (Ruby's non-nil values are all truthy, so the
            //     RHS branch is unreachable when LHS is statically
            //     non-null)
            //
            // For `And`, the result-of-a-truthy-chain idiom is less
            // common; keep the literal `&&` for bool LHS and otherwise
            // fall back to evaluating LHS-then-RHS via Rust's `if let`
            // shape would be involved — for now keep the literal form
            // and let bool cases work; non-bool `And` is exotic enough
            // to surface separately.
            if matches!(op, crate::expr::BoolOpKind::Or) {
                let lhs_is_option = matches!(
                    left.ty.as_ref(),
                    Some(crate::ty::Ty::Union { variants }) if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                );
                let lhs_is_bool = matches!(left.ty.as_ref(), Some(crate::ty::Ty::Bool));
                if lhs_is_option {
                    // `hash[k] || default` — the body-typer types
                    // `hash[k]` as `Option<V>` (nil-on-miss), but
                    // rust2 emits `Send { method: "[]" }` as the Rust
                    // `Index` form `hash[k]` (panic-on-miss, returns
                    // `&V`). The Or rewrite's `.unwrap_or(default)`
                    // would call unwrap_or on a `V`, not Option<V>.
                    // Detect the pattern and emit
                    //   `recv.get(k).cloned().unwrap_or(default)`
                    // directly — actually produces Option<V> the body-
                    // typer promised. Peel the recv's Union<Hash, Nil>
                    // (module-singleton ivars get widened by the flow-
                    // typer).
                    if let ExprNode::Send { recv: Some(r), method, args, .. } = &*left.node {
                        if method.as_str() == "[]"
                            && args.len() == 1
                            && matches!(
                                r.ty.as_ref().map(peel_nil),
                                Some(crate::ty::Ty::Hash { .. })
                            )
                        {
                            let recv_s = emit_expr(r);
                            let key_s = emit_expr(&args[0]);
                            // Hash<_, Untyped> recv → `.get(k).cloned()`
                            // returns `Option<serde_json::Value>`. A
                            // primitive default (literal int / bool /
                            // string) needs `serde_json::Value::from(...)`
                            // to type-unify. Closes the
                            // `attrs[:id] || 0` shape in lowered model
                            // bodies (`self.id = attrs[:id] || 0` →
                            // `self.set_id(attrs.get("id").cloned()
                            // .unwrap_or(serde_json::Value::from(0)))`).
                            let value_ty_untyped = matches!(
                                r.ty.as_ref().map(peel_nil),
                                Some(crate::ty::Ty::Hash { value, .. })
                                    if matches!(value.as_ref(), crate::ty::Ty::Untyped)
                            );
                            // String default literal -> `.to_string()`
                            // so unwrap_or's arg type matches the
                            // Option's inner type (HashMap<String, String>
                            // → Option<String>, default must be String).
                            // Defer to `analyze::str_color` when it's
                            // already annotated the literal (Phase 2 tail
                            // propagation from the BoolOp's surrounding
                            // expectation); double-applying produces
                            // `("").to_string().to_string()`.
                            let default_s = if value_ty_untyped {
                                coerce_to_value_default(right, emit_expr(right))
                            } else {
                                match &*right.node {
                                    ExprNode::Lit { value: Literal::Str { .. } }
                                        if right.str_coercion.is_none() =>
                                    {
                                        format!("{}.to_string()", emit_expr(right))
                                    }
                                    _ => emit_expr(right),
                                }
                            };
                            return format!(
                                "{recv_s}.get({key_s}).cloned().unwrap_or({default_s})"
                            );
                        }
                    }
                    // `Option<Untyped>` (Value) `||` literal — the
                    // default needs to be `Value`-shaped. Closes the
                    // `form_class || "button_to"` shape and any other
                    // Option<Value>.unwrap_or(<primitive>) site.
                    let lhs_inner_untyped = matches!(
                        left.ty.as_ref().map(peel_nil),
                        Some(crate::ty::Ty::Untyped)
                    );
                    let rhs_s = emit_expr(right);
                    let default_s = if lhs_inner_untyped {
                        coerce_to_value_default(right, rhs_s)
                    } else {
                        rhs_s
                    };
                    return format!(
                        "{}.unwrap_or({})",
                        emit_expr(left),
                        default_s,
                    );
                }
                if !lhs_is_bool && left.ty.is_some() {
                    // Statically non-nil — RHS is unreachable in Ruby
                    // semantics. Drop it.
                    return emit_expr(left);
                }
            }
            let op_s = match op {
                crate::expr::BoolOpKind::And => "&&",
                crate::expr::BoolOpKind::Or => "||",
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        // `case scrutinee; when Pat; body; …; end` → Rust `match`.
        // Used by the model lowerer's `synth_index_read` /
        // `synth_index_write` (get_index / set_index), which dispatch
        // on a Symbol-typed `name` param against per-column literal
        // patterns. The scrutinee's rust2 storage is `&str` (Sym
        // params lower to `&str`), so Sym-literal patterns emit as
        // `"name"` string literals.
        //
        // Wildcard arm: synthesized based on the enclosing return
        // type — `Value::Null` for `Value`-returning fns
        // (`get_index`), `()` for unit-returning fns (`set_index`).
        // Without an `_` arm, the match isn't exhaustive over `&str`
        // and Rust rejects with E0004.
        //
        // For `Value`-returning fns each arm's body is a concrete
        // primitive (an Ivar read of `String`/`i64`/etc.); wrap with
        // `serde_json::Value::from(...)` so the match unifies on
        // `Value` regardless of which arm fired.
        ExprNode::Case { scrutinee, arms } => {
            let scrutinee_s = emit_expr(scrutinee);
            let return_ty = CURRENT_RETURN_TY.with(|c| c.borrow().clone());
            let return_is_value = matches!(return_ty.as_ref(), Some(crate::ty::Ty::Untyped));
            let arm_strs: Vec<String> = arms
                .iter()
                .map(|arm| {
                    let pat_s = emit_case_pattern(&arm.pattern);
                    // Emit the arm body via `emit_expr_tail` so the
                    // Ivar arm sees `IN_RETURN_TAIL=true` and adds
                    // `.clone()` for non-Copy fields. Without that,
                    // `Value::from(self.body)` in the wrapped form
                    // below moves out of `&self.body` (E0507).
                    let body_s = emit_expr_tail(&arm.body);
                    let body_wrapped = if return_is_value
                        && !arm_body_already_value(&arm.body)
                    {
                        format!("serde_json::Value::from({body_s})")
                    } else {
                        body_s
                    };
                    format!("        {pat_s} => {{ {body_wrapped} }}")
                })
                .collect();
            let default_arm = if return_is_value {
                "serde_json::Value::Null".to_string()
            } else {
                "()".to_string()
            };
            format!(
                "match {scrutinee_s} {{\n{}\n        _ => {default_arm},\n    }}",
                arm_strs.join(",\n"),
            )
        }
        // `Cast { value, target_ty }` — explicit type narrowing the
        // model lowerer emits at adapter-row sites. The lowerer's
        // `synth_from_row` wraps each `row.<col>` accessor with a
        // Cast to the column's declared type; `synth_index_write`
        // wraps the per-arm `value` (column-union → emits as
        // `serde_json::Value` in rust2) with a Cast to the column
        // type so `@<col> = value.as(T)` gets the concrete shape.
        //
        // First try the body-typer-aware `coerce_arg_for_field_ty`;
        // if that returns the raw value unchanged AND the target is
        // a primitive AND the value's rust2-emit type is Value
        // (Untyped OR multi-variant non-Nilable Union), apply the
        // Value→primitive coercion explicitly. The body-typer's
        // Union-of-columns Ty doesn't peel to Untyped, but rust2
        // renders it as `serde_json::Value` at the param site —
        // `value.as(i64)` then needs `.as_i64().unwrap()`.
        ExprNode::Cast { value, target_ty } => {
            let coerced = coerce_arg_for_field_ty(value, target_ty);
            let raw = emit_expr(value);
            if coerced != raw {
                coerced
            } else if let Some(c) = cast_via_value_for_union(value, target_ty) {
                c
            } else {
                coerced
            }
        }
        // Catch-all for IR shapes not yet implemented. Each new runtime
        // file in Phase 2 expands this until full coverage.
        other => format!("/* TODO rust2: ExprNode::{:?} */", std::mem::discriminant(other)),
    }
}

/// Detect a standalone Ruby guard-clause on a Var/param:
///   return X if name.nil?
/// (or `raise X if name.nil?`). The body-typer narrows `name` to
/// non-nil for subsequent statements via the diverging-then narrowing
/// in Seq, but in Rust source `name` is still `Option<T>` from its
/// parameter declaration / earlier let. Emit
///   let Some(name) = name else { <then-branch> };
/// which rebinds `name` to the unwrapped value — matches the body-
/// typer's narrowing without changing the param signature.
///
/// Distinct from `try_fuse_let_else`: that helper handles `let x =
/// OPT; if x.nil? { ... }` (assign-then-guard). This one handles a
/// guard alone, where the unwrapped binding is a function param or
/// previously-introduced local.
fn try_emit_param_guard_unwrap(guard: &Expr) -> Option<(String, String)> {
    use crate::ty::Ty;
    let ExprNode::If { cond, then_branch, else_branch } = &*guard.node else {
        return None;
    };
    let ExprNode::Send { recv: Some(cond_recv), method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "nil?" || !args.is_empty() {
        return None;
    }
    let ExprNode::Var { name: var_name, .. } = &*cond_recv.node else {
        return None;
    };
    let recv_is_option = matches!(
        cond_recv.ty.as_ref(),
        Some(Ty::Union { variants }) if variants.iter().any(|v| matches!(v, Ty::Nil))
    );
    if !recv_is_option {
        return None;
    }
    let then_diverges = matches!(then_branch.ty.as_ref(), Some(Ty::Bottom));
    let else_is_nil = matches!(
        &*else_branch.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    if !then_diverges || !else_is_nil {
        return None;
    }
    let diverge_s = emit_expr_tail(then_branch);
    let n = var_name.as_str().to_string();
    Some((
        n.clone(),
        format!("let Some({n}) = {n} else {{ {diverge_s} }}"),
    ))
}

/// Detect the Ruby idiom
///   x = OPT
///   return ... if x.nil?
/// (or `raise ... if x.nil?`) — two adjacent Seq statements where the
/// first assigns a local from an Option-typed expression and the
/// second is a guard `if` whose then-branch diverges and whose
/// else-branch is empty. Emit as
///   let Some(x) = <opt> else { <then-branch> };
/// The body-typer's flow-narrowing already proves the rest of the
/// block sees `x` as non-nil; let-else gives Rust the same shape
/// without an extra `.unwrap()` rebind.
fn try_fuse_let_else(assign: &Expr, guard: &Expr) -> Option<(String, String)> {
    use crate::ty::Ty;
    // Stmt 0 must be a let assignment to a local Var whose RHS has
    // Option-shaped type (Union<T, Nil>).
    let ExprNode::Assign { target, value } = &*assign.node else {
        return None;
    };
    let LValue::Var { name: assign_name, .. } = target else {
        return None;
    };
    let value_is_option = matches!(
        value.ty.as_ref(),
        Some(Ty::Union { variants }) if variants.iter().any(|v| matches!(v, Ty::Nil))
    );
    if !value_is_option {
        return None;
    }

    // Stmt 1 must be an If whose:
    //   - cond is `<assign_name>.nil?`
    //   - then-branch diverges (typed Bottom — Return/Raise produce that)
    //   - else-branch is Nil-shaped (empty, the `if cond; ...; end` form)
    let ExprNode::If { cond, then_branch, else_branch } = &*guard.node else {
        return None;
    };
    let ExprNode::Send { recv: Some(cond_recv), method, args, .. } = &*cond.node else {
        return None;
    };
    if method.as_str() != "nil?" || !args.is_empty() {
        return None;
    }
    let ExprNode::Var { name: cond_name, .. } = &*cond_recv.node else {
        return None;
    };
    if cond_name != assign_name {
        return None;
    }
    let then_diverges = matches!(then_branch.ty.as_ref(), Some(Ty::Bottom));
    let else_is_nil = matches!(
        &*else_branch.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    if !then_diverges || !else_is_nil {
        return None;
    }

    let value_s = emit_expr(value);
    // The then-branch is divergent — its emit shape is a Return/Raise
    // statement. `emit_expr_tail` produces e.g. `return None` or
    // `panic!(...)`; either works as the body of a let-else block.
    let diverge_s = emit_expr_tail(then_branch);
    let n = assign_name.as_str().to_string();
    Some((
        n.clone(),
        format!("let Some({n}) = {value_s} else {{ {diverge_s} }}"),
    ))
}


/// Wrap a literal/Var default with `serde_json::Value::from(...)`
/// when it's going to be passed to an `unwrap_or` on an
/// `Option<serde_json::Value>`. Skip the wrap when the expression
/// already produces a `Value`. Used by the BoolOp::Or peepholes for
/// `hash[k] || default` against `Hash<_, Untyped>` recvs (lowered
/// model bodies' `attrs[:col] || 0` style) and the
/// Option<Untyped>.unwrap_or-literal catch-all.
fn coerce_to_value_default(default_expr: &Expr, raw: String) -> String {
    use crate::ty::Ty;
    let primitive = matches!(
        default_expr.ty.as_ref(),
        Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool)
    ) || matches!(
        &*default_expr.node,
        ExprNode::Lit {
            value: Literal::Str { .. }
                | Literal::Sym { .. }
                | Literal::Int { .. }
                | Literal::Float { .. }
                | Literal::Bool { .. }
        }
    );
    if primitive {
        format!("serde_json::Value::from({raw})")
    } else {
        raw
    }
}

/// If `arg` is a Var (possibly wrapped in `.clone()`) with a
/// recorded Hash local_var_ty, return (K, V). Used by the
/// `Self::method` callee-back-propagation in emit_send: when the
/// callee's param is `Hash<_, Untyped>` we need to know the arg's
/// local-typed shape to decide whether to insert the value-coercion
/// transform.
pub(super) fn arg_hash_var_local_ty(arg: &Expr) -> Option<(crate::ty::Ty, crate::ty::Ty)> {
    // Peel one `.clone()` shell: the rust2 auto-clone-on-multi-read
    // path produces `name.clone()` for some Var reads.
    let inner: &Expr = match &*arg.node {
        ExprNode::Send { recv: Some(r), method, args, .. }
            if method.as_str() == "clone" && args.is_empty() =>
        {
            r
        }
        _ => arg,
    };
    let name = match &*inner.node {
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        _ => return None,
    };
    match local_var_ty(&name)? {
        crate::ty::Ty::Hash { key, value } => Some((*key, *value)),
        _ => None,
    }
}

/// If `recv` is a Var whose `local_var_ty` was set via
/// back-propagation (`empty_hash_return_ty`), return its (K, V)
/// types. Gated on the back-propagation set so the Send `[]=`
/// peephole only coerces args when the recorded type is
/// authoritative — body-typer-derived types may disagree with the
/// emit's actual storage (e.g. `Hash<Sym, Str>` in IR but `HashMap<
/// &str, String>` in emit for a `{action: …, method: "post"}.to_h`
/// literal).
pub(super) fn recv_var_back_propagated_hash_kv(recv: &Expr) -> Option<(crate::ty::Ty, crate::ty::Ty)> {
    let name = match &*recv.node {
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        _ => return None,
    };
    let is_back_propagated =
        BACK_PROPAGATED_HASH_LOCALS.with(|c| c.borrow().contains(&name));
    if !is_back_propagated {
        return None;
    }
    match local_var_ty(&name)? {
        crate::ty::Ty::Hash { key, value } => Some((*key, *value)),
        _ => None,
    }
}



/// Snapshot the DECLARED_VARS set, run `f`, then restore the snapshot.
/// Used around each If/While/loop branch's body emit so a `let x = …`
/// inside one branch doesn't suppress the `let` on a fresh `x = …` in
/// the next branch or after the if. Rust scopes are per-block; the
/// emit tracker mirrors that with this stack-like wrap.
pub(super) fn with_declared_vars_scope<R>(f: impl FnOnce() -> R) -> R {
    let snapshot = DECLARED_VARS.with(|c| c.borrow().clone());
    let r = f();
    DECLARED_VARS.with(|c| *c.borrow_mut() = snapshot);
    r
}

/// True when the branch's tail expression — after walking through a
/// trailing `Seq` — is a Var read whose recorded `local_var_ty` is
/// already `Option<T>`. Used by the tail-position Some-wrap to avoid
/// re-wrapping an Option-shaped value into `Option<Option<T>>` (the
/// belongs_to-method body's `result` accumulator pattern: rust2's
/// `none_init_option_return_ty` back-prop typed `let mut result:
/// Option<Article> = None`, so the Seq's tail Var read already
/// produces `Option<Article>`).
///
/// Walks one Seq level (the lowerer's belongs_to bodies wrap the
/// adapter-find sequence in a single Seq); nested Seqs aren't
/// expected. Returns false on any non-Var tail to keep the Some-wrap
/// firing for the literal/expression branch shapes that genuinely
/// need it (`if x then 5 else nil end`-style).
fn tail_produces_option(branch: &Expr) -> bool {
    // Identify the tail Var name (single-level Seq peel, plus direct
    // `Var`-as-branch). LOCAL_VAR_TYPES at this point has already
    // been scope-restored by the Seq's `with_rebound_vars_scope`, so
    // we can't read it back — instead walk the Seq's stmts for the
    // `Assign { Var{name}, Nil }` pattern that triggered
    // `none_init_option_return_ty`'s back-prop during emit. When that
    // assign is present AND the function returns `Option<_>`, the
    // emitted let-binding annotated the var as `Option<T>` and every
    // subsequent rebind landed there — so the tail Var read produces
    // `Option<T>`.
    let (tail_name, exprs) = match &*branch.node {
        ExprNode::Seq { exprs } => match exprs.last() {
            Some(last) => match &*last.node {
                ExprNode::Var { name, .. } => (Some(name.as_str().to_string()), exprs.as_slice()),
                _ => return false,
            },
            None => return false,
        },
        ExprNode::Var { name, .. } => (Some(name.as_str().to_string()), &[] as &[Expr]),
        _ => return false,
    };
    let Some(name) = tail_name else { return false };
    if !current_return_is_option() {
        return false;
    }
    // Single-Var-as-branch can't carry the init pattern alone — the
    // Var was bound somewhere in the enclosing scope. Without local_
    // var_ty we don't know whether it's Option-typed; play it safe
    // and assume it is (matches the lowerer's accumulator shape).
    if exprs.is_empty() {
        return true;
    }
    exprs.iter().any(|e| matches!(
        &*e.node,
        ExprNode::Assign {
            target: crate::expr::LValue::Var { name: assign_name, .. },
            value,
        } if assign_name.as_str() == name
            && matches!(&*value.node, ExprNode::Lit { value: Literal::Nil })
    ))
}


