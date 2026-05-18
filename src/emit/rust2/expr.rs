//! `rust2` expression emit — `Expr` IR → Rust source-text.
//!
//! Phase 2.1 scope: minimal handling for the inflector body shape
//! (Lit, Var, Send `==`, StringInterp, If). Extended file-by-file
//! through Phase 2 as each runtime file forces new IR shapes.

use crate::expr::{Expr, ExprNode, InterpPart, LValue, Literal};

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

fn local_var_ty(name: &str) -> Option<crate::ty::Ty> {
    LOCAL_VAR_TYPES.with(|c| c.borrow().get(name).cloned())
}

fn mark_local_var_ty(name: &str, ty: crate::ty::Ty) {
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
fn class_method_param_ty(method: &str, idx: usize) -> Option<crate::ty::Ty> {
    CLASS_METHOD_PARAM_TYS
        .with(|c| c.borrow().get(method).and_then(|tys| tys.get(idx).cloned()))
}

/// Return the full Vec of positional param Tys for a method in the
/// current class. Used by the Const-recv dispatch to check arity
/// + pad missing trailing args with defaults — Ruby's `def
/// initialize(attrs = {})` accepts zero-arg `Article.new`, but
/// Rust requires the explicit `HashMap::new()` default.
fn current_class_method_param_tys(method: &str) -> Option<Vec<crate::ty::Ty>> {
    CLASS_METHOD_PARAM_TYS
        .with(|c| c.borrow().get(method).cloned())
}

/// Emit a Case `Pattern` as a Rust `match` arm pattern. The
/// lowerer-synthesized `synth_index_read`/`synth_index_write` use
/// `Pattern::Lit { value: Symbol }` against an `&str`-typed
/// scrutinee — emit as a string-literal pattern. Other shapes fall
/// through to `_` until they're needed.
fn emit_case_pattern(p: &crate::expr::Pattern) -> String {
    use crate::expr::Pattern;
    match p {
        Pattern::Wildcard => "_".to_string(),
        Pattern::Lit { value } => match value {
            Literal::Str { value } => format!("{value:?}"),
            Literal::Sym { value } => format!("{:?}", value.as_str()),
            Literal::Int { value } => value.to_string(),
            Literal::Bool { value } => value.to_string(),
            Literal::Nil => "_".to_string(),
            _ => "_".to_string(),
        },
        Pattern::Bind { name } => name.as_str().to_string(),
        _ => "_".to_string(),
    }
}

/// True when an arm body's emit is already `Value`-shaped (Ivar
/// read in a class whose field is typed `Untyped`, or a Send
/// already wrapped with `Value::from`). Conservative — over-wraps
/// won't cause a type error since `Value::from(Value)` doesn't
/// impl, so we only skip the wrap on the shapes that emit_expr
/// has already coerced.
fn arm_body_already_value(body: &Expr) -> bool {
    matches!(body.ty.as_ref(), Some(crate::ty::Ty::Untyped))
        || matches!(
            &*body.node,
            ExprNode::Lit { value: Literal::Nil }
        )
}

/// Synthesize a default-value Rust expression for a missing arg
/// position. Mirrors the Ruby default-arg semantics for the
/// shapes the lowerer-synthesized constructors use (`attrs = {}`
/// → `HashMap::new()`).
fn synth_default_for_ty(ty: &crate::ty::Ty) -> Option<String> {
    use crate::ty::Ty;
    match ty {
        Ty::Hash { .. } => Some("std::collections::HashMap::new()".to_string()),
        Ty::Array { .. } => Some("vec![]".to_string()),
        Ty::Str | Ty::Sym => Some("String::new()".to_string()),
        Ty::Int => Some("0_i64".to_string()),
        Ty::Float => Some("0.0_f64".to_string()),
        Ty::Bool => Some("false".to_string()),
        Ty::Untyped => Some("serde_json::Value::Null".to_string()),
        _ => None,
    }
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

fn in_return_tail() -> bool {
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

/// Conservative `Copy`-trait check for Rust target types. Numeric +
/// bool + nil are Copy; String/Vec/HashMap/Option/Class are not.
/// Used by the `Ivar` arm to decide whether a tail-position read
/// needs `.clone()` to avoid moving out of `&self`. `Ty::Untyped`
/// commits to `serde_json::Value` which is non-Copy.
fn is_copy_ty(t: &crate::ty::Ty) -> bool {
    use crate::ty::Ty;
    // Sym maps to `String` in rust2 (see `ty.rs::rust_ty`), so it's
    // non-Copy despite being a primitive-shaped Ruby type.
    matches!(t, Ty::Int | Ty::Bool | Ty::Nil | Ty::Float)
}

fn is_static_method(name: &str) -> bool {
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

/// Indent every line of `s` by `level` four-space blocks. Used for
/// nested-block rendering (while/for loop bodies, future for-loops,
/// etc.); top-level method-body indent is handled by the caller in
/// `method.rs`.
fn indent(s: &str, level: usize) -> String {
    let pad = "    ".repeat(level);
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("{pad}{l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_hash(entries: &[(Expr, Expr)]) -> String {
    // Empty hash (`@data = {}` in HWIA initialize) → fresh HashMap.
    // The empty-literal shape is the canonical accumulator init in
    // Rails source; non-empty literals appear later (Parameters
    // builders, view_helpers DEFAULTS) and need richer emit.
    if entries.is_empty() {
        return "std::collections::HashMap::new()".to_string();
    }
    // Tuple-type unification: `HashMap::from([(k, v), ...])` infers
    // its key/value types from the FIRST tuple. Subsequent tuples
    // must share that type exactly. The Ruby body-typer's view of
    // `{ type: "submit", value: text }` is uniformly `Hash<Sym, Str>`,
    // but the rust2 emit treats `Ty::Str` as `String` while string
    // literals default to `&'static str` — heterogeneous tuples
    // result. Force every string-literal value to `String` (via
    // `.to_string()`) when ANY entry's value is non-literal-string
    // (the case that proves the surrounding map is String-typed).
    // Keys stay as-is — they're typically all literal and Rust's
    // `Borrow<&str>` lets `HashMap<&str, _>` accept owned String
    // keys at the merge_attrs boundary without forcing per-key
    // coercion.
    let has_non_literal_str_value = entries.iter().any(|(_, v)| {
        !matches!(&*v.node, ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } })
            && matches!(v.ty.as_ref(), Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym))
    });
    // Tail-position return-type coercion: when the literal is the
    // method body's tail AND the declared return is `Hash<String, V>`
    // (lowerer-synthesized `attributes` returning `HashMap<String,
    // Value>` is the canonical case), coerce keys to String and
    // values to V's storage. Without this, `HashMap::from([("body",
    // self.body), ("article_id", self.article_id), …])` infers from
    // the first tuple's value type (String, or i64) and trips
    // E0308 on the heterogeneous values and at the return site.
    let return_hash_kv: Option<(crate::ty::Ty, crate::ty::Ty)> = if in_return_tail() {
        match CURRENT_RETURN_TY.with(|c| c.borrow().clone()) {
            Some(crate::ty::Ty::Hash { key, value }) => Some((*key, *value)),
            _ => None,
        }
    } else {
        None
    };
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| {
            // Defer to `analyze::str_color` when it's already
            // annotated the literal — the pass owns this coercion
            // (Phase 2.4 hash homogeneity); double-applying produces
            // `("post").to_string().to_string()`.
            let str_color_handled = v.str_coercion.is_some();
            let v_raw = emit_expr(v);
            let v_s = if let Some((_, ref v_ty)) = return_hash_kv {
                // Use the per-param coercion helper — handles
                // Value::from for Untyped-target values, .to_string()
                // for Str-target values, etc.
                coerce_arg_for_param_ty(v, v_ty)
            } else if !str_color_handled
                && has_non_literal_str_value
                && matches!(&*v.node, ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } })
            {
                format!("{v_raw}.to_string()")
            } else {
                v_raw
            };
            let k_raw = emit_expr(k);
            let k_s = if let Some((ref k_ty, _)) = return_hash_kv {
                match k_ty {
                    crate::ty::Ty::Str | crate::ty::Ty::Sym
                        if matches!(
                            &*k.node,
                            ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                        ) && k.str_coercion.is_none() =>
                    {
                        format!("{k_raw}.to_string()")
                    }
                    _ => k_raw,
                }
            } else {
                k_raw
            };
            format!("({k_s}, {v_s})")
        })
        .collect();
    format!("std::collections::HashMap::from([{}])", pairs.join(", "))
}

/// Build a Rust closure literal `|params| body` from a Lambda IR
/// node. Single-line bodies inline; multi-line bodies wrap in
/// `{ ... }`. No type annotations on params — call-site inference
/// handles them in the cases we actually hit; explicit types come
/// later when generic Lambda usage forces them.
fn emit_closure(params: &[crate::ident::Symbol], body: &Expr) -> String {
    let ps: Vec<String> = params.iter().map(|p| p.to_string()).collect();
    let body_s = emit_expr(body);
    if body_s.contains('\n') {
        format!(
            "|{}| {{\n{}\n}}",
            ps.join(", "),
            indent(&body_s, 1),
        )
    } else {
        format!("|{}| {{ {body_s} }}", ps.join(", "))
    }
}

/// Append a block-as-closure to a `recv.method(...)` call. The
/// block's Lambda IR carries params + body; we emit a closure
/// literal and splice it as the last arg. Empty arg lists become
/// single-arg (`recv.method(|...| ...)`); non-empty lists insert
/// the closure after the existing args. Detection of "method
/// shouldn't take a closure" (e.g. mapping `each` to `iter()`
/// stdlib chains) is per-target work for later.
fn attach_block(base: &str, block: &Expr) -> String {
    let closure = if let ExprNode::Lambda { params, body, .. } = &*block.node {
        emit_closure(params, body)
    } else {
        // Non-Lambda block — shouldn't appear in lowered IR, but
        // emit something recognizable rather than panic.
        format!("/* TODO rust2: non-Lambda block: {:?} */", std::mem::discriminant(&*block.node))
    };
    // `base` is shaped as `recv.method(args)` or `name(args)`. The
    // closing `)` is the last char; insert the closure before it
    // (with a leading `, ` when args are already present).
    if let Some(stripped) = base.strip_suffix("()") {
        format!("{stripped}({closure})")
    } else if let Some(stripped) = base.strip_suffix(')') {
        format!("{stripped}, {closure})")
    } else {
        // Defensive — base didn't end as a call; just append.
        format!("{base}({closure})")
    }
}

/// `recv.is_a?(Class)` → serde_json predicate where the class
/// name maps to a Value variant, else `false` with a marker
/// comment. Detection: the arg's IR shape is `Const { path }`
/// (the class reference); the last segment is the name we map.
fn emit_is_a(recv: &Expr, class_arg: &Expr) -> String {
    let class_name = match &*class_arg.node {
        ExprNode::Const { path } => path.last().map(|s| s.to_string()).unwrap_or_default(),
        _ => return format!("/* is_a? unknown class: {} */ false", emit_expr(class_arg)),
    };
    let recv_s = emit_expr(recv);
    // serde_json::Value variants: Null, Bool, Number, String, Array,
    // Object. Map the Ruby stdlib class names that the runtime files
    // actually use.
    let predicate = match class_name.as_str() {
        "Hash" => Some("is_object"),
        "Array" => Some("is_array"),
        "String" => Some("is_string"),
        "Integer" => Some("is_i64"),
        "Float" => Some("is_f64"),
        "TrueClass" | "FalseClass" => Some("is_boolean"),
        "NilClass" => Some("is_null"),
        _ => None,
    };
    match predicate {
        Some(p) => format!("{recv_s}.{p}()"),
        None => format!("/* is_a?({class_name}): no Value variant */ false"),
    }
}

fn emit_array(elements: &[Expr]) -> String {
    // `vec![]` works for both empty and populated literals; lets the
    // surrounding type context infer the element type. The macro form
    // is the Rust idiom for `Vec<T>` literals and matches how the
    // emitted runtime files actually want to build their state.
    //
    // Tail-position return-type coercion: when this Array literal is
    // the method body's tail value AND the declared return is
    // `Vec<String>` / `Vec<Sym>` (e.g. lowerer-synthesized
    // `schema_columns` returning `Vec<String>` from a
    // `vec!["id", "body", …]` of `&'static str` literals), coerce each
    // entry to `String` via `.to_string()`. Without this the literal
    // infers as `Vec<&str>` and the return-site type-checks fail.
    let coerce_to_string_elem = in_return_tail()
        && CURRENT_RETURN_TY.with(|c| {
            matches!(
                c.borrow().as_ref(),
                Some(crate::ty::Ty::Array { elem })
                    if matches!(elem.as_ref(), crate::ty::Ty::Str | crate::ty::Ty::Sym)
            )
        });
    let parts: Vec<String> = elements
        .iter()
        .map(|e| {
            let raw = emit_expr(e);
            if coerce_to_string_elem
                && matches!(
                    &*e.node,
                    ExprNode::Lit {
                        value: Literal::Str { .. } | Literal::Sym { .. }
                    }
                )
                && e.str_coercion.is_none()
            {
                format!("{raw}.to_string()")
            } else {
                raw
            }
        })
        .collect();
    format!("vec![{}]", parts.join(", "))
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
fn arg_hash_var_local_ty(arg: &Expr) -> Option<(crate::ty::Ty, crate::ty::Ty)> {
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
fn recv_var_back_propagated_hash_kv(recv: &Expr) -> Option<(crate::ty::Ty, crate::ty::Ty)> {
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

/// Return `Hash<K, V>` when `value` is the empty `{}` literal AND
/// the enclosing function's declared return type is `Hash<K, V>` (or
/// `Option<Hash<K, V>>`). `None` otherwise. The back-propagated type
/// pins a local HashMap's K/V before subsequent inserts narrow them
/// in the wrong direction (`&str` keys + values from local borrowed
/// source data). See `match_pattern` in
/// `runtime/ruby/action_dispatch/router.rb`: `params = {}; params[
/// pp[1..]] = ap; …; return params`. Used both for the let
/// annotation AND for subsequent `.insert`-arg coercion via
/// `local_var_ty`.
fn empty_hash_return_ty(value: &Expr) -> Option<crate::ty::Ty> {
    let is_empty_hash = matches!(
        &*value.node,
        ExprNode::Hash { entries, .. } if entries.is_empty()
    );
    if !is_empty_hash {
        return None;
    }
    let ret_ty = CURRENT_RETURN_TY.with(|c| c.borrow().clone());
    match ret_ty {
        Some(crate::ty::Ty::Hash { key, value }) => Some(crate::ty::Ty::Hash { key, value }),
        Some(crate::ty::Ty::Union { variants }) => variants.into_iter().find(|v| {
            matches!(v, crate::ty::Ty::Hash { .. })
        }),
        _ => None,
    }
}

/// Wrap an RHS with `Some(...)` when the variable's recorded
/// `local_var_ty` is `Option<T>` and the RHS produces non-Option
/// `T` (or anything coercible into Option<_>). Used by emit_assign
/// for reassign sites where `let mut result = None;` (Option<X>)
/// is followed by `result = instance` (X) inside a conditional —
/// the lowerer-synthesized accumulator pattern in `_adapter_find_*`.
///
/// `self` rhs is special-cased: the method takes `&self`/`&mut self`
/// so `self` is a reference. Owning the inner type for `Option<T>`
/// requires `.clone()`, which the function-tail-return path already
/// does via `needs_self_clone`; we mirror that here.
fn some_wrap_for_assign(name: &str, value: &Expr, rhs: &str) -> String {
    let Some(declared) = local_var_ty(name) else {
        return rhs.to_string();
    };
    if !is_option_ty(&declared) {
        return rhs.to_string();
    }
    // RHS already Option-shaped? Skip the wrap.
    let rhs_is_option = value
        .ty
        .as_ref()
        .map(|t| is_option_ty(t))
        .unwrap_or(false);
    if rhs_is_option {
        return rhs.to_string();
    }
    // `self` rhs in a `&self`/`&mut self` method: `Some(self)` would
    // produce `Option<&Self>`. The lowered `_adapter_save` shape
    // (`result = self` inside the if-step branch) wants the owned
    // `Option<Self>`, so clone.
    if matches!(&*value.node, ExprNode::SelfRef) {
        return format!("Some({rhs}.clone())");
    }
    format!("Some({rhs})")
}

fn empty_hash_return_annotation(value: &Expr) -> String {
    match empty_hash_return_ty(value) {
        Some(crate::ty::Ty::Hash { key, value }) => format!(
            ": std::collections::HashMap<{}, {}>",
            super::ty::rust_ty(&key),
            super::ty::rust_ty(&value),
        ),
        _ => match none_init_option_return_ty(value) {
            Some(t) => format!(": {}", super::ty::rust_ty(&t)),
            None => String::new(),
        },
    }
}

/// `Option<T>` (or the unioned `Union<T, Nil>`) when `value` is the
/// `nil` literal AND the enclosing function returns `Option<T>`.
/// Mirrors `empty_hash_return_ty` for the lowerer's `_adapter_find_
/// by_id` accumulator pattern: `result = nil; if step? ; result =
/// instance ; end ; result`. Without the annotation, Rust infers the
/// `let mut result = None` binding from the first non-None
/// reassignment, which produces a concrete `Article` instead of the
/// declared `Option<Article>` and trips E0308 at every reassign +
/// the return.
fn none_init_option_return_ty(value: &Expr) -> Option<crate::ty::Ty> {
    let is_nil_lit = matches!(
        &*value.node,
        ExprNode::Lit { value: Literal::Nil }
    );
    if !is_nil_lit {
        return None;
    }
    let ret_ty = CURRENT_RETURN_TY.with(|c| c.borrow().clone());
    match ret_ty {
        Some(t) if is_option_ty(&t) => Some(t),
        _ => None,
    }
}

fn emit_assign(target: &LValue, value: &Expr) -> String {
    let rhs = emit_expr(value);
    match target {
        LValue::Var { name, .. } => {
            let name_str = name.as_str().to_string();
            // Track local-var declared type for the narrowing-aware
            // Var read (params = match_pattern(...) → Option<HashMap>,
            // then `unless params.nil?; ...; params; end` reads need
            // `.clone().unwrap()`). Only records on first assignment —
            // subsequent rebinds in the same Seq leave the recorded
            // declared type alone (Rust's `mut` binding type is fixed).
            //
            // For empty-HashMap inits in Hash-returning functions, the
            // back-propagated `Hash<K, V>` ty takes precedence over the
            // body-typer's `Hash<Untyped, Untyped>` view — subsequent
            // `.insert` emits use this to coerce args to the right
            // K/V color.
            if local_var_ty(&name_str).is_none() {
                let back_propagated = empty_hash_return_ty(value)
                    .or_else(|| none_init_option_return_ty(value));
                if back_propagated.is_some() {
                    BACK_PROPAGATED_HASH_LOCALS.with(|c| {
                        c.borrow_mut().insert(name_str.clone());
                    });
                }
                let ty = back_propagated.or_else(|| value.ty.clone());
                if let Some(t) = ty {
                    mark_local_var_ty(&name_str, t);
                }
            }
            let already_declared =
                DECLARED_VARS.with(|c| c.borrow().contains(&name_str));
            if already_declared {
                // Some-wrap when the binding was declared `Option<T>`
                // (typically `let mut result = None;`) and the new
                // RHS is plain `T`. Without this, `result = instance.
                // clone()` after `result = None` fails E0308. Same
                // shape as the function-tail Some-wrap but applied
                // at the LValue::Var assign site, which catches the
                // lowerer-synthesized `result = instance; ...;
                // result` accumulator pattern in
                // `_adapter_find_by_id` / `find` and friends.
                let rhs_wrapped = some_wrap_for_assign(&name_str, value, &rhs);
                return format!("{name_str} = {rhs_wrapped}");
            }
            let needs_mut = MUT_VARS.with(|c| c.borrow().contains(&name_str));
            // Type-annotate empty HashMap literals when the enclosing
            // function returns a Hash<K, V> (or Option<Hash<K, V>>). The
            // pattern: `params = {}; ...; params[k] = v; ...; return
            // params`. Without an annotation, Rust infers params' type
            // from the FIRST `.insert(k, v)` — often `HashMap<&str,
            // &str>` when the source values are `&str`-typed locals,
            // which then mismatches the function's declared `Hash<
            // String, String>?` return. Annotating up front pins the
            // K/V types so str_color's Assign-Index coercion fires on
            // the inserts and the return-site type-checks.
            //
            // Heuristic: empty-HashMap init + Hash-returning function.
            // False positive — the local is never returned — is benign:
            // the annotation matches what the inserts produce in
            // practice (the str_color coercion lifts &str → String).
            // Over-applying is preferable to under-applying because
            // the wrong inference fails noisily.
            let annot = empty_hash_return_annotation(value);
            DECLARED_VARS.with(|c| {
                c.borrow_mut().insert(name_str.clone());
            });
            if needs_mut {
                format!("let mut {name_str}{annot} = {rhs}")
            } else {
                format!("let {name_str}{annot} = {rhs}")
            }
        }
        LValue::Ivar { name } => {
            // Field-type coercion: when RHS is a `Ty::Str` value (a
            // string literal, an `&str`-typed param/var, an expression
            // returning &str) and the declared field is `Ty::Str` /
            // `Ty::Sym` (both render as `String`), wrap RHS with
            // `.to_string()`. Without this, `self.body = ""` (literal
            // `""` is `&str`, field is `String`) fails E0308. Inside
            // the constructor the same lookup also annotates the
            // let-binding type so the closing `Self { ... }` literal
            // gets a `String` slot rather than `&str`.
            let rhs_coerced = maybe_to_string_coercion(name.as_str(), value, &rhs);
            if in_module_singleton() {
                // Module-singleton ivar write — route through the
                // static Mutex slot (`*ADAPTER.lock().unwrap() =
                // Some(value)`). Always Some-wraps so the slot stays
                // `Option<T>` regardless of T's nullability — read-
                // side defaults handle the "not yet set" case.
                let slot = module_singleton_slot_name(name.as_str());
                return format!(
                    "*{slot}.lock().unwrap() = Some({rhs_coerced})"
                );
            }
            if in_constructor() {
                // Annotate the let with the field's declared type so
                // the closing `Self { f1, f2, ... }` literal sees
                // matching types. Without the annotation, a `let mut
                // body = ""` declared as `&str` collides with the
                // `String`-typed field at the Self literal site.
                let annot = field_let_annotation(name.as_str());
                return format!("let mut {name}{annot} = {rhs_coerced}");
            }
            format!("self.{name} = {rhs_coerced}")
        }
        LValue::Attr { recv, name } => {
            // `self.x = ...` inside a module-singleton class method
            // refers to the class itself, not an instance — route
            // through the static slot (same path as the Ivar branch
            // above). Other Attr LHS forms (`obj.field = ...` on a
            // non-self receiver) keep the simple field-assignment
            // emit.
            if in_module_singleton() && matches!(&*recv.node, ExprNode::SelfRef) {
                let slot = module_singleton_slot_name(name.as_str());
                return format!("*{slot}.lock().unwrap() = Some({rhs})");
            }
            format!("{}.{name} = {rhs}", emit_expr(recv))
        }
        LValue::Index { recv, index } => {
            // Module-singleton Ivar `[]=`: `@slots[k] = v` in a
            // `def self.foo` body needs to mutate the static
            // `Mutex<Option<HashMap>>` slot through
            // `get_or_insert_with` — not the cloned snapshot the
            // default Ivar-read emit returns. Otherwise the write
            // lands on a temporary that's dropped before the
            // surrounding statement finishes; the mutation is lost
            // (silent runtime bug) and Rust catches the surface
            // `&str` vs `String` mismatch via the `HashMap<String,
            // String>` value type.
            //
            // Key + value get `.to_string()` appended unconditionally
            // — view_helpers' `@slots[slot] = value` passes `&str`
            // for both, and str_color's Hash-recv-K/V coloring
            // doesn't fire here (the body-typer types `@slots` from
            // the empty `{}` init as `Hash<Untyped, Untyped>`, not
            // the RBS-declared `Hash<Symbol, String>`). The append is
            // idempotent on already-String shapes.
            // `recv[k] = v` on a Flash / Session struct dispatches to
            // the hand-written `.set(key, value)` method (no
            // IndexMut impl; the runtime/rust/flash.rs etc. surface
            // explicit setters).
            if let Some(crate::ty::Ty::Class { id, .. }) = recv.ty.as_ref() {
                let cls = id.0.as_str();
                if matches!(cls, "Flash" | "ActionDispatch::Flash")
                {
                    // Flash::set takes `Option<String>` (per
                    // runtime/rust/flash.rs:47). Session::set takes
                    // bare `String`. Wrap a non-Option-shaped rhs in
                    // `Some(...)` so the narrowed-Var emit
                    // (`notice.clone().unwrap()`) reaches a typed slot.
                    let rhs_is_option = matches!(
                        value.ty.as_ref(),
                        Some(crate::ty::Ty::Union { variants })
                            if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                    );
                    let wrapped = if rhs_is_option {
                        rhs.clone()
                    } else {
                        format!("Some({rhs})")
                    };
                    return format!(
                        "{}.set({}, {wrapped})",
                        emit_expr(recv),
                        emit_expr(index),
                    );
                }
                if matches!(cls, "Session" | "ActionDispatch::Session")
                {
                    return format!(
                        "{}.set({}, {rhs})",
                        emit_expr(recv),
                        emit_expr(index),
                    );
                }
                // Other Ty::Class receivers (ActiveRecord::Base and
                // friends) route through the `set_index` method emit
                // (per the operator-method rewrite in `sanitize_ident`).
                // Wrap String RHS with `serde_json::Value::from` because
                // `def []=` is declared `(Symbol, untyped) -> untyped`
                // (RBS in `active_record/base.rbs`), which renders the
                // value param as `serde_json::Value`. Already-Value RHS
                // pass through; non-built-in classes get the method
                // call; `Hash` / `Array` / built-in containers fall
                // through to the bracket-assignment emit below.
                if !is_builtin_container_class(cls) {
                    let coerced_rhs = coerce_to_value(value, &rhs);
                    return format!(
                        "{}.set_index({}, {coerced_rhs})",
                        emit_expr(recv),
                        emit_expr(index),
                    );
                }
            }
            // HashMap doesn't implement `IndexMut` — `recv[k] = v`
            // requires `.insert(k, v)`. The Ruby source's
            // `hash[key] = value` lowers as an `Assign { target:
            // LValue::Index }` and pre-rust2 emit used the bracket
            // form, which rustc rejects (E0594 "cannot assign to data
            // in an index"). Recv-aware dispatch: Ty::Hash → `.insert`,
            // other built-in containers (Array indexable by usize)
            // keep the bracket-assign form for now.
            //
            // Wrap in `{ ...; }` so the assignment evaluates to `()`.
            // `HashMap::insert` returns `Option<V>` (previous value)
            // which rustc rejects in `if cond { recv[k]=v }` no-else
            // contexts (E0317). Trailing `;` inside the block discards.
            if matches!(recv.ty.as_ref(), Some(crate::ty::Ty::Hash { .. })) {
                return format!(
                    "{{ {}.insert({}, {rhs}); }}",
                    emit_expr(recv),
                    emit_expr(index),
                );
            }
            format!("{}[{}] = {rhs}", emit_expr(recv), emit_expr(index))
        }
    }
}

/// Coerce RHS expressions to the declared field type when emit
/// produces a known-incompatible shape. Two cases land here:
///
///   1. `&str` → `String`: when the named ivar's declared field
///      type is `Ty::Str` (or `Ty::Sym`, both render as `String`)
///      and the RHS is `Ty::Str`/`Ty::Sym`, append `.to_string()`.
///      Without this, `self.body = ""` (literal `""` is `&str`,
///      field is `String`) fails E0308.
///
///   2. `T` → `Option<T>`: when the field type is `Ty::Union {Nil, T}`
///      (renders as `Option<T>`) and the RHS isn't itself an Option
///      (its Ty isn't a Nil-containing Union), wrap with `Some(...)`.
///      Most commonly `self.location = path` where path is `&str`
///      and the field is `Option<String>`. Combines with the
///      String coercion when T is Str (Some(rhs.to_string())).
///
/// Other RHS / field combinations pass through unchanged.
fn maybe_to_string_coercion(ivar_name: &str, value: &Expr, rhs: &str) -> String {
    let Some(field_ty) = ivar_field_ty(ivar_name) else {
        return rhs.to_string();
    };
    // Unwrap Option-typed field — track whether we need to add the
    // Some() wrap at the end. Pulls in any combination of
    // `T → Option<T>` and the underlying String coercion below.
    let (inner_field_ty, needs_some) = match &field_ty {
        crate::ty::Ty::Union { variants } if variants.len() == 2 => {
            let nil_idx = variants.iter().position(|v| matches!(v, crate::ty::Ty::Nil));
            match nil_idx {
                Some(0) => (variants[1].clone(), true),
                Some(1) => (variants[0].clone(), true),
                _ => (field_ty.clone(), false),
            }
        }
        _ => (field_ty.clone(), false),
    };
    // Authoritative RHS Ty: prefer the body-typer's `value.ty` when
    // set — it reflects flow-sensitive narrowing (e.g. `content_type`
    // inside `unless content_type.nil?` narrowed from `Option<String>`
    // to `String`). Falling back to `param_ty(name)` would re-broaden
    // the type to the RBS-declared (un-narrowed) param ty, which then
    // triggers a spurious `.unwrap()` chain at the coercion site
    // (action_controller_base render's E0599). The param-table fallback
    // remains for callers that bypass the body-typer or land here
    // before typing (value.ty == None).
    let effective_value_ty = match &*value.node {
        ExprNode::Var { name, .. } => value.ty.clone().or_else(|| param_ty(name.as_str())),
        _ => value.ty.clone(),
    };
    // If the RHS is itself an Option (Union{Nil, _}), it already
    // matches the Option-typed field — don't re-wrap.
    let rhs_is_option = matches!(
        effective_value_ty.as_ref(),
        Some(crate::ty::Ty::Union { variants }) if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
    );
    // Three coercion paths:
    //   1. `T → T`               — emit unchanged
    //   2. `Ty::Str → String`    — handled by `analyze::str_color`
    //                              when `value.str_coercion` is set
    //                              (the new pass owns Ivar-assign
    //                              ownership annotation since Phase
    //                              2.1). Fall back to the legacy
    //                              `.to_string()` peephole only when
    //                              the pass produced no annotation —
    //                              covers callers that bypass the
    //                              pass (e.g. app-level code not yet
    //                              colored).
    //   3. `Option<T> → T`       — append `.unwrap()` (Ruby idiom
    //                              `@x = y unless y.nil?` lowers the
    //                              body to plain `@x = y`; the guard
    //                              proves Some at the assignment site)
    let str_color_handled = value.str_coercion.is_some();
    let coerced = if !str_color_handled
        && matches!(inner_field_ty, crate::ty::Ty::Str | crate::ty::Ty::Sym)
        && matches!(effective_value_ty.as_ref(), Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym))
    {
        format!("{rhs}.to_string()")
    } else if !needs_some && rhs_is_option {
        // Field is non-Option but RHS is Option-typed — unwrap.
        // Generated code paths into this branch run only after an
        // `if x.is_none() return / skip` guard (the Ruby `unless
        // x.nil?` idiom); unwrap is safe in that flow.
        format!("{rhs}.unwrap()")
    } else {
        rhs.to_string()
    };
    if needs_some && !rhs_is_option && !matches!(effective_value_ty.as_ref(), Some(crate::ty::Ty::Nil)) {
        format!("Some({coerced})")
    } else {
        coerced
    }
}

/// In constructor mode, render the type annotation for the let
/// binding that backs the named ivar. Returns `: <Ty>` when the
/// field type is known, empty string otherwise (let inference
/// covers the unknown-type case).
fn field_let_annotation(ivar_name: &str) -> String {
    match ivar_field_ty(ivar_name) {
        Some(ty) => format!(": {}", super::ty::rust_ty(&ty)),
        None => String::new(),
    }
}

fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    // Constructor `self.field = value` rewrite: `pub fn new(...) ->
    // Self` has no `self` until the closing `Self { ... }` literal,
    // but the lowerer-synthesized `def initialize` body emits
    // `Send { recv: SelfRef, method: "<field>=" }` calls (matching
    // Ruby's `self.col = attrs[:col]` shape). Emit as `let <field>
    // = <value>` so the closing struct literal's shorthand binding
    // picks up the local of the same name. `collect_ivars_assigned_
    // in_body` recognizes this same pattern (instead of just
    // LValue::Ivar / LValue::Attr assigns) so the default-init pass
    // skips fields the body already set.
    if in_constructor()
        && args.len() == 1
        && method.ends_with('=')
        && !method.starts_with('[')
    {
        if let Some(r) = recv {
            if matches!(&*r.node, ExprNode::SelfRef) {
                let field = &method[..method.len() - 1];
                // Coerce the value to the struct field's declared
                // type so the closing `Self { ... }` literal's
                // shorthand binding picks up a same-typed local.
                // Field-position coercion differs from param-position:
                // String fields want owned `String` (not the `&str`
                // that `.as_str().unwrap()` produces for &str params).
                let rhs = match ivar_field_ty(field) {
                    Some(fty) => coerce_arg_for_field_ty(&args[0], &fty),
                    None => emit_expr(&args[0]),
                };
                return format!("let {field} = {rhs}");
            }
        }
    }
    // Stdlib class-method bridges. The Ruby source's `Time.now.utc.iso8601`,
    // `Base64.strict_encode64(...)`, `JSON.generate(...)` patterns
    // refer to Ruby stdlib that doesn't exist in Rust. Recognize the
    // Const-typed receiver and emit the equivalent crate call. The
    // `regex`, `base64`, and `chrono` crates are already rust2 deps.
    if let Some(r) = recv {
        if let ExprNode::Const { path } = &*r.node {
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            match (last, method, args.len()) {
                ("Time", "now", 0) => {
                    return "chrono::Utc::now()".to_string();
                }
                ("JSON", "generate" | "dump" | "fast_generate", 1) => {
                    return format!("serde_json::to_string(&{}).unwrap()", emit_expr(&args[0]));
                }
                ("JSON", "pretty_generate", 1) => {
                    return format!(
                        "serde_json::to_string_pretty(&{}).unwrap()",
                        emit_expr(&args[0])
                    );
                }
                ("Base64", "encode64" | "strict_encode64", 1) => {
                    return format!(
                        "{{ use base64::Engine; base64::engine::general_purpose::STANDARD.encode({}) }}",
                        emit_expr(&args[0])
                    );
                }
                ("Base64", "urlsafe_encode64", 1) => {
                    return format!(
                        "{{ use base64::Engine; base64::engine::general_purpose::URL_SAFE.encode({}) }}",
                        emit_expr(&args[0])
                    );
                }
                _ => {}
            }
        }
        // `.utc()` on a `Ty::Class { Time }` recv (already a chrono
        // DateTime<Utc> after `Time.now`) is a no-op — chrono's
        // `Utc::now()` is already UTC. `.iso8601()` becomes
        // `.to_rfc3339()`; `.strftime(fmt)` becomes `.format(fmt).to_string()`.
        if matches!(
            r.ty.as_ref().map(peel_nil),
            Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Time"
        ) {
            match (method, args.len()) {
                ("utc" | "to_time", 0) => return emit_expr(r),
                ("iso8601" | "rfc3339", 0) => return format!("{}.to_rfc3339()", emit_expr(r)),
                ("rfc2822", 0) => return format!("{}.to_rfc2822()", emit_expr(r)),
                ("strftime", 1) => {
                    return format!(
                        "{}.format({}).to_string()",
                        emit_expr(r),
                        emit_expr(&args[0])
                    );
                }
                _ => {}
            }
        }
    }
    // Binary operators (==, !=, <, >, +, -, *, /) ingest as Send
    // with `method` as the operator name. Ruby `a == b` lowers to
    // `Send { recv: a, method: ==, args: [b] }`.
    // Ruby's `+` on strings concatenates; Rust's `&str + &str`
    // doesn't compile (need owned LHS), and `"a" + b + "c"` chains
    // would need cascading allocations. Emit string concatenation
    // as `format!("{}{}", a, b)` — handles every (&str, &str),
    // (&str, String), (String, &str), and chained-format!s as
    // single allocations through format_args!. Recv-type-aware: only
    // fires on Ty::Str/Ty::Sym receivers; numeric `+` keeps its
    // binary-operator emit below.
    if method == "+"
        && recv.is_some()
        && args.len() == 1
        && matches!(
            recv.unwrap().ty.as_ref(),
            Some(crate::ty::Ty::Str) | Some(crate::ty::Ty::Sym)
        )
    {
        return format!(
            "format!(\"{{}}{{}}\", {}, {})",
            emit_expr(recv.unwrap()),
            emit_expr(&args[0]),
        );
    }
    if matches!(method, "==" | "!=" | "<" | ">" | "<=" | ">=" | "+" | "-" | "*" | "/")
        && recv.is_some()
        && args.len() == 1
    {
        return format!("{} {} {}", emit_expr(recv.unwrap()), method, emit_expr(&args[0]));
    }
    // Unary `!` — `!cond` in Ruby lowers as `Send { recv: cond,
    // method: "!", args: [] }`. Rust uses the same `!` operator
    // syntactically but as a prefix unary, not a method call.
    if method == "!" && args.is_empty() {
        if let Some(r) = recv {
            return format!("!{}", emit_expr(r));
        }
    }
    // Array append: `arr << x` Ruby idiom → `arr.push(x)` in Rust.
    // Recv-type-aware: only fires for Vec/Array-typed receivers so
    // user-defined `<<` operators on other types stay intact. The
    // arg is coerced into the elem type so push() type-checks:
    // Vec<String>::push wants owned `String`, but the body-typer
    // often hands us &str literals or borrowed `&str`. `<<` is
    // value-semantic in Ruby (Array#<< takes any object, no borrow
    // distinction), so eager `.to_string()` on the literal side is
    // the right Rust analog. Other elem types pass through unchanged
    // — when more callers surface, extend this match arm in place.
    if method == "<<" && args.len() == 1 {
        if let Some(r) = recv {
            if let Some(crate::ty::Ty::Array { elem }) = r.ty.as_ref() {
                let arg_rendered = match (elem.as_ref(), args[0].ty.as_ref()) {
                    (crate::ty::Ty::Str | crate::ty::Ty::Sym, Some(crate::ty::Ty::Str | crate::ty::Ty::Sym)) => {
                        format!("({}).to_string()", emit_expr(&args[0]))
                    }
                    _ => emit_expr(&args[0]),
                };
                return format!("{}.push({})", emit_expr(r), arg_rendered);
            }
        }
    }
    // Index access: `recv[k]` / `recv[k] = v`. The lowerer shapes
    // both as `Send` with method `[]` / `[]=`; Rust uses the
    // brackets-as-operator form via the `Index` trait. `[]=` lands
    // here for cases not caught by `Assign { target: LValue::Index }`
    // — most commonly `@data[k] = v` (the Ivar-recv case is `Send`
    // because the lowerer hasn't synthesized an LValue::Index for it).
    if let Some(r) = recv {
        if method == "[]" && args.len() == 1 {
            // Peel `Union<T, Nil>` from the recv Ty so receivers bound
            // via `let x = arr[i]` (typed `T | Nil` by the body-typer's
            // Ruby-semantics view) match the same branches as the
            // plain receiver case. Emit chose panic-on-miss for `[]`,
            // so the runtime value really is T.
            let recv_ty = r.ty.as_ref().map(peel_nil);
            let arg_ty = args[0].ty.as_ref().map(peel_nil);
            // Range index on Str/Vec receiver — `pp[1..]`. The Range
            // node emits its endpoints unmodified (`1_i64..`), but
            // slice indexing needs `usize`. Wrap the rendered range
            // in a re-cast so `&pp[(1) as usize..]` compiles.
            if matches!(
                recv_ty,
                Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
                    | Some(crate::ty::Ty::Array { .. })
            ) {
                if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                    let b = begin
                        .as_ref()
                        .map(|e| format!("({}) as usize", emit_expr(e)))
                        .unwrap_or_default();
                    let e = end
                        .as_ref()
                        .map(|e| format!("({}) as usize", emit_expr(e)))
                        .unwrap_or_default();
                    let op = if *exclusive { ".." } else { "..=" };
                    let range_s = match (begin.is_some(), end.is_some()) {
                        (true, true) => format!("{b}{op}{e}"),
                        (true, false) => format!("{b}.."),
                        (false, true) => format!("..{e}"),
                        (false, false) => "..".to_string(),
                    };
                    // Str slices need a `&` prefix to yield `&str`; Vec
                    // slices likewise yield `&[T]`. Either way the
                    // caller treats it as borrowed.
                    return format!("&{}[{range_s}]", emit_expr(r));
                }
            }
            // Negative-int literal index on Vec/Str (`arr[-1]` = last
            // element, `arr[-2]` = second to last). Rust's `Index`
            // panics on negative-cast-to-usize (`(-1_i64) as usize`
            // is a huge number). Rewrite to `recv[recv.len() - N]`
            // where N is the absolute negative. Mirrors the TS emit
            // (`recv[recv.length-N]`) at line ~1600 of typescript/expr.rs.
            // Only fires for literal negatives — dynamic negative
            // indices would need a runtime branch, which the framework
            // patterns we ship today don't require.
            if matches!(
                recv_ty,
                Some(crate::ty::Ty::Array { .. })
                    | Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
            ) {
                if let ExprNode::Lit { value: Literal::Int { value } } = &*args[0].node {
                    if *value < 0 {
                        let recv_s = emit_expr(r);
                        let abs = -value;
                        // Vec<T>::Index returns `&T`; clone to produce
                        // the owned `T` Ruby's `arr[-1]` semantics
                        // delivers. Callers (e.g. `Base.last`'s tail
                        // wrapped in `Some(...)`) need owned T to
                        // match the `Option<T>` return type.
                        if matches!(recv_ty, Some(crate::ty::Ty::Array { .. })) {
                            return format!(
                                "{recv_s}[{recv_s}.len() - {abs}_usize].clone()"
                            );
                        }
                        return format!("{recv_s}[{recv_s}.len() - {abs}_usize]");
                    }
                }
            }
            // Slice/Vec indexing needs `usize`. Ruby integers (including
            // numeric loop counters like `let mut i = 0_i64`) lower to
            // `i64`; without a cast the `Index<i64>` impl is missing
            // and rustc rejects with E0277. Recv-type-aware: only
            // fires when recv is Ty::Array and the index expression's
            // Ty is Ty::Int. HashMap indexing keeps the bare form
            // (HashMap<K, V> indexes by &K, the user-supplied key
            // is already the right type).
            if let Some(crate::ty::Ty::Array { elem }) = recv_ty {
                if matches!(arg_ty, Some(crate::ty::Ty::Int)) {
                    // `Vec<T>::Index` returns `&T`; passing the result
                    // to a function taking `T` by value (the typical
                    // Ruby-emit consuming-arg shape) requires
                    // materializing an owned T. Append `.clone()` for
                    // non-Copy element types — mirrors the negative-
                    // index branch's clone (the same E0507 motivates
                    // both). Copy elems (i64, f64, bool) need no
                    // suffix.
                    let suffix = if is_copy_ty(elem) { "" } else { ".clone()" };
                    return format!(
                        "{}[({}) as usize]{}",
                        emit_expr(r),
                        emit_expr(&args[0]),
                        suffix
                    );
                }
            }
            // Ty::Class non-builtin recv: route `recv[k]` through the
            // `get_index` method emitted by `sanitize_ident`'s `[]` →
            // `get_index` rewrite. Built-in containers (Hash, Array)
            // and HWIA / Flash / Session keep the bracket form.
            if let Some(crate::ty::Ty::Class { id, .. }) = recv_ty {
                let cls = id.0.as_str();
                if !is_builtin_container_class(cls) {
                    return format!(
                        "{}.get_index({})",
                        emit_expr(r),
                        emit_expr(&args[0])
                    );
                }
            }
            return format!("{}[{}]", emit_expr(r), emit_expr(&args[0]));
        }
        // Ruby `String#[](start, length)` — byte-slice with start +
        // length. Ruby's substring semantics are owned (a fresh
        // `String` each call), so the emit produces owned `String`
        // via `.to_string()` on the slice. Without ownership, a
        // pattern like `ms = padded[0, 3]` reassigns an outer-scope
        // binding to a slice of an inner-scope local (`padded` drops
        // at end of the if-block), tripping E0597. Producing String
        // at the slice site matches Ruby semantics + lets the
        // multi-assign coordination in `analyze::str_color` line up
        // the binding's declared type (`let mut ms: String = …`)
        // with subsequent slice assignments. Args land here as `i64`
        // from the body-typer, hence the `as usize` casts. Caveat:
        // `start_s` is duplicated in the emitted source; fine for
        // the literal/local arg shapes seen in practice (`str[0,
        // 10]`, `str[0, cutoff]`).
        if method == "[]" && args.len() == 2 {
            let recv_s = emit_expr(r);
            let start_s = emit_expr(&args[0]);
            let len_s = emit_expr(&args[1]);
            return format!(
                "(&{recv_s}[({start_s}) as usize..(({start_s}) + ({len_s})) as usize]).to_string()"
            );
        }
        if method == "[]=" && args.len() == 2 {
            // Module-singleton Ivar `[]=`: `@slots[k] = v` in a
            // `def self.foo` body needs to mutate the static
            // `Mutex<Option<HashMap>>` slot through
            // `get_or_insert_with`. The default Ivar-read emit
            // returns a cloned snapshot; bracket-writing into that
            // clone is a silent runtime bug AND fails the surface
            // type-check (`HashMap<String, String>` value type vs
            // `&str` arg, which is what `view_helpers.rs:76/92`
            // reported). Key/value get `.to_string()` appended —
            // the body-typer types `@slots` from the `{}` init as
            // `Hash<Untyped, Untyped>`, so the str_color Hash-K/V
            // coloring doesn't fire here; an unconditional append
            // is idempotent on already-String shapes.
            if in_module_singleton() {
                if let ExprNode::Ivar { name } = &*r.node {
                    let slot = module_singleton_slot_name(name.as_str());
                    let k = emit_expr(&args[0]);
                    let v = emit_expr(&args[1]);
                    return format!(
                        "{{ {slot}.lock().unwrap().get_or_insert_with(std::collections::HashMap::new).insert(({k}).to_string(), ({v}).to_string()); }}"
                    );
                }
            }
            // Mirror the LValue::Index Flash/Session bridge — when
            // the Send-shape `recv.[]=(k, v)` lands on a Flash or
            // Session typed receiver, route through the hand-written
            // `.set(key, value)` method (no IndexMut impl). Per-app
            // model classes implement column-specific `[]=` overrides
            // via the IR's regular method path, not this branch.
            //
            // Type detection covers two channels: `recv.ty` (when the
            // body-typer set it) and an `Ivar { name }` recv whose
            // declared field type sits in IVAR_TYPES.
            let recv_class = match r.ty.as_ref() {
                Some(crate::ty::Ty::Class { id, .. }) => Some(id.0.as_str().to_string()),
                _ => match &*r.node {
                    ExprNode::Ivar { name } => match ivar_field_ty(name.as_str()) {
                        Some(crate::ty::Ty::Class { id, .. }) => {
                            Some(id.0.as_str().to_string())
                        }
                        _ => None,
                    },
                    _ => None,
                },
            };
            if let Some(cls) = recv_class.as_deref() {
                if matches!(cls, "Flash" | "ActionDispatch::Flash") {
                    // Flash::set takes Option<String>; wrap non-Option
                    // rhs in Some(...). See the LValue::Index Flash
                    // branch above — same shape, just the Send-form
                    // entry point (`@flash.[]=(k, v)` in the lowered IR).
                    let rhs = emit_expr(&args[1]);
                    let rhs_is_option = matches!(
                        args[1].ty.as_ref(),
                        Some(crate::ty::Ty::Union { variants })
                            if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                    );
                    let wrapped = if rhs_is_option {
                        rhs
                    } else {
                        format!("Some({rhs})")
                    };
                    return format!(
                        "{}.set({}, {wrapped})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                    );
                }
                if matches!(cls, "Session" | "ActionDispatch::Session") {
                    return format!(
                        "{}.set({}, {})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                        emit_expr(&args[1]),
                    );
                }
                // Non-builtin Ty::Class — route through `set_index`
                // (the `[]=` operator-method rewrite). Wrap value RHS
                // with Value::from when its Ty isn't already
                // serde_json::Value-shaped.
                if !is_builtin_container_class(cls) {
                    let rhs = emit_expr(&args[1]);
                    let coerced_rhs = coerce_to_value(&args[1], &rhs);
                    return format!(
                        "{}.set_index({}, {coerced_rhs})",
                        emit_expr(r),
                        emit_expr(&args[0]),
                    );
                }
            }
            // Ty::Hash recv via Send-`[]=` — HashMap doesn't implement
            // IndexMut, so `recv[k] = v` syntax fails (E0594). Mirror
            // the LValue::Index Ty::Hash branch and route through
            // `.insert(k, v)`. Wrap in `{ ...; }` for the no-else
            // `if cond { ... }` context (HashMap::insert returns
            // Option<V>; trailing `;` makes the block `()` typed).
            //
            // K/V coercion via `local_var_ty`: when the recv is a Var
            // whose locally-tracked type (e.g. back-propagated from
            // the function's return signature, see
            // `empty_hash_return_ty`) is `Hash<Str|Sym, Str|Sym>`, the
            // body-typer's IR-level `r.ty` may still be the
            // `Hash<Untyped, Untyped>` shape from the `{}` init — so
            // str_color's Hash-recv coloring doesn't fire and the args
            // arrive un-coerced. Append `.to_string()` per arg whose
            // K/V slot is String-shaped. Idempotent for already-String.
            let recv_hash_kv = recv_var_back_propagated_hash_kv(r);
            let is_hash_recv =
                matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
                    || recv_hash_kv.is_some();
            if is_hash_recv {
                let coerce_str = |slot: Option<&crate::ty::Ty>, raw: String| -> String {
                    match slot {
                        Some(crate::ty::Ty::Str | crate::ty::Ty::Sym) => {
                            format!("({raw}).to_string()")
                        }
                        _ => raw,
                    }
                };
                let k = emit_expr(&args[0]);
                let v = emit_expr(&args[1]);
                let (kk, vv) = match &recv_hash_kv {
                    Some((k_ty, v_ty)) => (
                        coerce_str(Some(k_ty), k),
                        coerce_str(Some(v_ty), v),
                    ),
                    None => (k, v),
                };
                return format!("{{ {}.insert({kk}, {vv}); }}", emit_expr(r));
            }
            return format!("{}[{}] = {}", emit_expr(r), emit_expr(&args[0]), emit_expr(&args[1]));
        }
        // Ruby `value.is_a?(Class)` runtime type check. Rust has no
        // generic analog — every type is statically known. For the
        // `serde_json::Value`-typed gradual-escape recv (the common
        // shape after Ty::Untyped commits to `serde_json::Value`),
        // map the known Ruby class names to serde_json predicates;
        // user-defined classes degrade to `false` with a comment
        // (always-false branch in a chain like normalize_value, the
        // next branch handles the real case).
        if method == "is_a?" && args.len() == 1 {
            return emit_is_a(r, &args[0]);
        }
        // `hash.to_h` — Ruby identity on Hash (returns self). Crystal
        // uses it to bridge NamedTuple → Hash; Rust has no NamedTuple,
        // so on a HashMap-typed recv the method has no analog and
        // `.clone()` preserves the "fresh owned hash" semantics. The
        // Flash/Session typed structs have their own `to_h()` method
        // returning HashMap<String, String>; those go through the
        // generic dispatch path below (recv ty is not Ty::Hash there).
        // Recv typed Untyped/None falls through too — call sites that
        // need the .to_h() on a serde_json::Value will rely on
        // separate emit work for the Value method-call fix.
        if method == "to_h"
            && args.is_empty()
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
        {
            return format!("{}.clone()", emit_expr(r));
        }
        // `hash.merge(other)` — Ruby Hash#merge returns a new Hash
        // with `other`'s entries layered on top. Rust HashMap has no
        // built-in merge AND the typical call site has mixed K/V
        // types (literal `(&str, &str)` merged with parameter
        // `HashMap<String, Value>`), which a generic trait can't
        // bridge. `merge_attrs` from runtime/rust/hash_ext.rs takes
        // both sides as IntoIterator over (K: Into<String>, V:
        // Into<Value>) and produces a unified
        // `HashMap<String, Value>` — matches what `render_attrs`,
        // `r#where`, and similar consumers expect. Recv-types
        // outside Ty::Hash (Flash, Session) keep their own `merge`
        // method via the generic dispatch path below.
        if method == "merge"
            && args.len() == 1
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Hash { .. }))
        {
            return format!(
                "merge_attrs({}, {})",
                emit_expr(r),
                emit_expr(&args[0]),
            );
        }
        // `hash.fetch(key, default)` — Ruby Hash#fetch returns the
        // value at key or `default` when missing. Rust HashMap has
        // `.get(K) -> Option<&V>`; bridge via `.cloned()` (nil
        // default; `Option::None` unifies trivially) or
        // `.cloned().unwrap_or(default)` otherwise. Recv must be a
        // Ty::Hash that's a real value at the call site.
        // Const receivers (e.g. `STATUS_CODES.fetch(...)`) are
        // accepted now that module-level Hash constants emit as
        // `static NAME: LazyLock<HashMap<...>>` — the LazyLock
        // auto-deref makes `STATUS_CODES.get(&k)` resolve through to
        // the inner HashMap.
        if method == "fetch"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Hash { .. }))
        {
            let recv_s = emit_expr(r);
            let key_s = emit_expr(&args[0]);
            let default_is_nil = matches!(
                &*args[1].node,
                ExprNode::Lit { value: Literal::Nil }
            );
            if default_is_nil {
                return format!("{recv_s}.get({key_s}).cloned()");
            }
            let default_s = emit_expr(&args[1]);
            return format!("{recv_s}.get({key_s}).cloned().unwrap_or({default_s})");
        }
        // `s.tr(from, to)` — character translation. Limited to
        // single-char from/to (the framework Ruby use case is
        // `inner_k.to_s.tr("_", "-")` in render_attrs). Multi-char
        // and ranges fall through to generic dispatch so the gap
        // surfaces as a compile error rather than silently mis-
        // emitting. Mirrors the TypeScript emit's `tr` peephole.
        if method == "tr" && args.len() == 2 {
            if let (
                ExprNode::Lit { value: Literal::Str { value: from } },
                ExprNode::Lit { value: Literal::Str { value: to } },
            ) = (&*args[0].node, &*args[1].node)
            {
                if from.chars().count() == 1 && to.chars().count() == 1 {
                    let recv_s = emit_expr(r);
                    return format!("{recv_s}.replace({from:?}, {to:?})");
                }
            }
        }
        // `str.split(sep)` — Ruby returns an Array; Rust returns a
        // lazy `Split` iterator that doesn't support `.len()` or
        // indexing. Eagerly collect to Vec<&str> so downstream
        // `parts.length` / `parts[i]` (the typical router-table
        // walking pattern) compiles. Recv-type-aware: only fires on
        // Ty::Str receivers; user-defined `split` on other types
        // stays intact.
        if method == "split"
            && args.len() == 1
            && matches!(r.ty.as_ref(), Some(crate::ty::Ty::Str))
        {
            let recv_s = emit_expr(r);
            let sep_s = emit_expr(&args[0]);
            return format!("{recv_s}.split({sep_s}).collect::<Vec<&str>>()");
        }
        // `s.gsub(pattern, table)` with a `Ty::Class { Regexp }` first
        // arg and a `Ty::Hash` second arg — the canonical Ruby idiom
        // for table-driven character replacement (used by
        // `view_helpers.html_escape` and `json_builder.encode_string`).
        // Rust analog: `pattern.replace_all(&s, |caps| table[&caps[0]])`
        // returning the owned String. The pattern is typically a
        // module-level `LazyLock<Regex>` constant, the table a
        // `LazyLock<HashMap<&'static str, &'static str>>` — both auto-
        // deref to their inner types at the call site.
        if method == "gsub"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(
                args[0].ty.as_ref().map(peel_nil),
                Some(crate::ty::Ty::Class { id, .. }) if id.0.as_str() == "Regexp"
            )
            && matches!(args[1].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Hash { .. }))
        {
            let recv_s = emit_expr(r);
            let pat_s = emit_expr(&args[0]);
            let table_s = emit_expr(&args[1]);
            return format!(
                "{pat_s}.replace_all(&{recv_s}, |__caps: &regex::Captures| -> String \
                 {{ (*{table_s}.get(&__caps[0]).unwrap_or(&\"\")).to_string() }}).into_owned()"
            );
        }
        // `s.gsub(needle, replacement)` — both String args. Ruby
        // returns a new string with all occurrences substituted;
        // Rust's `str::replace(needle, replacement)` is the direct
        // analog (same all-occurrences semantics, returns owned String).
        if method == "gsub"
            && args.len() == 2
            && matches!(r.ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(args[0].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
            && matches!(args[1].ty.as_ref().map(peel_nil), Some(crate::ty::Ty::Str))
        {
            let recv_s = emit_expr(r);
            let needle_s = emit_expr(&args[0]);
            let repl_s = emit_expr(&args[1]);
            return format!("{recv_s}.replace({needle_s}, {repl_s})");
        }
        // `arr.length` / `str.length` — Ruby returns Integer.
        // Rust's `.len()` returns `usize`, but Ruby Integers lower
        // to `i64` everywhere else (`while i < arr.length`, `if
        // arr.length == 0`). Emit as `(recv.len() as i64)` on
        // sized receivers so downstream i64 arithmetic / comparison
        // compiles without a per-call-site widen. Untyped receivers
        // fall through to the generic `.length -> .len()` bridge
        // (their value-shape may not even support `.len()`).
        if method == "length"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(crate::ty::Ty::Array { .. })
                    | Some(crate::ty::Ty::Hash { .. })
                    | Some(crate::ty::Ty::Str)
                    | Some(crate::ty::Ty::Sym)
            )
        {
            return format!("({}.len() as i64)", emit_expr(r));
        }
        // Recv-Ty-aware method bridge — mirrors the structure of
        // `typescript/expr.rs`'s match-on-recv-ty around lines
        // 1972–2245. Each Ruby method on Array/Str/Sym/Hash gets a
        // Rust-shaped lowering; unrecognized names fall through to
        // the generic dispatch + rewrite_method_name table below.
        // Recv-aware so user-defined methods of the same name on
        // other types still resolve through the regular path.
        if let Some(rendered) = dispatch_method_by_recv_ty(r, method, args) {
            return rendered;
        }
        // `str.capitalize` — Ruby's "first letter uppercase, rest
        // lowercase". Rust's String has no direct analog; inline a
        // small block that chains uppercase-first + lowercase-rest.
        // Fires whenever `.capitalize()` is called with no args; the
        // body-typer doesn't always propagate Ty::Str through getter
        // Sends (e.g. `self.model_name.capitalize` where model_name
        // is an attr_reader returning String), so checking recv.ty
        // misses those cases. Non-String receivers would already
        // fail E0599 today and surface as a clearer error after the
        // bridge fires (`&recv_s` deref against a non-String type).
        if method == "capitalize" && args.is_empty() {
            let recv_s = emit_expr(r);
            return format!(
                "{{ let __s: &str = &{recv_s}; let mut __c = __s.chars(); \
                    match __c.next() {{ \
                        Some(__f) => __f.to_uppercase().collect::<String>() + &__c.as_str().to_lowercase(), \
                        None => String::new(), \
                    }} }}"
            );
        }
        // `value.nil?` on a `Ty::Untyped` or unresolved-Var receiver —
        // `serde_json::Value` exposes `.is_null()` (not `.is_none`,
        // which is the Option method the generic `nil?` bridge below
        // produces). The Var-typed case covers receivers the body-
        // typer didn't fully resolve (e.g. `value = @model[field]`
        // where `@model[field]` is typed Untyped per Base's RBS but
        // the local-let propagation leaves `value`'s recv ty
        // unresolved at the emit-walk's view of the Var-read site).
        // The generic bridge stays in place for true Option-typed
        // receivers (typical Ruby `attr_reader` getters typed `T?`).
        if method == "nil?"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(crate::ty::Ty::Untyped) | Some(crate::ty::Ty::Var { .. })
            )
        {
            return format!("{}.is_null()", emit_expr(r));
        }
        // `value.nil?` on a non-nilable primitive — body-typer says
        // `Ty::Str` / `Ty::Int` / etc. Ruby/Crystal need the runtime
        // check (Crystal's `property title : String?` is nilable; Ruby
        // attrs default to nil), but rust2's struct field is the bare
        // owned type with no `Option<...>` wrapper, so `.is_none()`
        // would E0599. The static answer here is `false` — emit a
        // constant and let LLVM fold the surrounding If. Side-effect-
        // free recvs (`self.title`) drop out; expression-shaped recvs
        // (a Send chain) need a tiny bind-and-discard, but the body
        // -typer's nil? targets are almost always Ivar reads. Inline
        // the bind only when the recv isn't a pure read.
        if method == "nil?"
            && args.is_empty()
            && matches!(
                r.ty.as_ref(),
                Some(
                    crate::ty::Ty::Str
                        | crate::ty::Ty::Sym
                        | crate::ty::Ty::Int
                        | crate::ty::Ty::Float
                        | crate::ty::Ty::Bool
                )
            )
        {
            let recv_pure = matches!(
                &*r.node,
                ExprNode::Ivar { .. }
                    | ExprNode::Var { .. }
                    | ExprNode::SelfRef
                    | ExprNode::Lit { .. }
                    | ExprNode::Const { .. }
            );
            if recv_pure {
                return "false".to_string();
            }
            return format!("{{ let _ = {}; false }}", emit_expr(r));
        }
        // `self.class.X(args)` — Ruby idiom for "dispatch X on the
        // class of self" (`@id` getter is an instance dispatch;
        // `table_name`, `schema_columns` are per-subclass class
        // methods). The chained Send `recv.class.X` lowers to
        // `Send { recv: Send { recv: SelfRef, method: "class" },
        // method: X }`. In Rust, the equivalent is `Self::X(args)`
        // — the surrounding `impl Base` resolves the per-class
        // override at the call site. Only fires when the recv is
        // SelfRef (subclass overrides reach the correct method
        // through Self resolution); other receivers' .class chains
        // surface as proper E0599 noise upstream.
        if let ExprNode::Send {
            recv: Some(inner_recv),
            method: inner_method,
            args: inner_args,
            ..
        } = &*r.node
        {
            if inner_method.as_str() == "class"
                && inner_args.is_empty()
                && matches!(&*inner_recv.node, ExprNode::SelfRef)
            {
                let rewritten = rewrite_method_name(method);
                let args_s: Vec<String> = args.iter().map(emit_expr).collect();
                if args_s.is_empty() {
                    return format!("Self::{rewritten}()");
                }
                return format!("Self::{rewritten}({})", args_s.join(", "));
            }
        }
    }
    // Ruby/Rust method-name bridge. Sanitize predicates (`foo?` →
    // `foo`, `foo!` → `foo`) since Rust identifiers reject those
    // suffixes. The user-defined HWIA methods `key?`/`has_key?`/etc.
    // pair with the matching `pub fn` rename in `method.rs` so def
    // and call sites stay aligned. A small set of Ruby stdlib calls
    // (`to_s`, `length`, `nil?`, `key?` on Hash, etc.) needs a
    // different Rust name; rewrite those here. Caveat: receiver-type-
    // sensitive bridges (Hash#key? vs user-defined `key?`) collapse
    // to the generic form — Rust's `contains_key` for HashMap vs
    // the user's stripped `key` may emit ambiguously when the recv
    // is untyped serde_json::Value. Live with the noise until type-
    // aware bridging lands.
    let rewritten_method = rewrite_method_name(method);
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Free functions / module functions (Inflector.pluralize → bare
    // pluralize() in the inflector module). Implicit-self bare calls
    // emit as bare function calls.
    if recv.is_none() {
        // `require "X"` inside a method body — Ruby's lazy load
        // statement. Rust resolves cross-file deps through top-level
        // `use` imports (the runtime_loader's `imports` field), so
        // the inline `require` has nothing to do at runtime. Emit as
        // a comment so the line stays inert.
        if method == "require" {
            let arg_repr = args_s.join(", ");
            return format!("/* require({arg_repr}) — no-op in rust2 */");
        }
        // Ruby's class-method `new` (implicit-self call to Class#new
        // inside a `def self.X` body). Lowers to `Send { recv: None,
        // method: "new" }`. Rust analog inside an `impl Type` is
        // `Self::new(args)` — the constructor's canonical Rust name
        // (matches `emit_instance_method`'s `is_init` lowering).
        if method == "new" && in_class_method() {
            return format!("Self::new({})", args_s.join(", "));
        }
        return format!("{}({})", rewritten_method, args_s.join(", "));
    }
    let r = recv.unwrap();
    // Static-method routing: `self.method(args)` where `method` was
    // classified as not-reading-self emits as `Self::method(args)`.
    // Required inside `pub fn new` (no instance yet), and also a
    // valid choice elsewhere for inherently-static helpers — Rust
    // accepts both `obj.foo()` and `T::foo(...)` when `foo` doesn't
    // take a receiver, but the static form is unambiguous.
    //
    // The same routing applies unconditionally inside class methods
    // (`def self.X` bodies): Ruby's `self` *is* the class there, so
    // every `self.method(args)` is class-level dispatch.
    if matches!(&*r.node, ExprNode::SelfRef) && (is_static_method(method) || in_class_method()) {
        // Callee-back-propagation: when the callee's declared param[i]
        // is `Hash<K, V>` and the arg expression is a Var whose
        // `local_var_ty` is a different `Hash<K', V'>` (or
        // body-typer-derived but with mismatched K/V), insert a
        // `into_iter().map().collect()` transform. The button_to →
        // render_attrs(form_attrs) pattern is the canonical case:
        // form_attrs is locally `HashMap<&str, String>` (from
        // `{action: …, method: "post"}.to_h`), render_attrs takes
        // `HashMap<String, serde_json::Value>`.
        let coerced: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, a)| coerce_arg_for_class_method(method, i, a))
            .collect();
        if coerced.is_empty() {
            return format!("Self::{rewritten_method}()");
        }
        return format!("Self::{rewritten_method}({})", coerced.join(", "));
    }
    // Callee-back-propagation for two recv shapes:
    //
    // 1. **SelfRef instance method** (`self.set_id(arg)`): callee is
    //    a sibling method on the current class. Use
    //    `CLASS_METHOD_PARAM_TYS` (populated by `library.rs` at class
    //    emit start) to look up the param Tys. Closes the lowered
    //    model `self.set_id(row["id"])` shape (Value → i64 coercion).
    // 2. **Const class method** (`Db::escape_string(self.body)`):
    //    callee is in a hand-written runtime module not surfaced
    //    through the per-class registry. Hardcoded
    //    `external_class_method_param_tys` covers Db today; future
    //    modules add entries as their sites surface.
    let final_args: Vec<String> = if matches!(&*r.node, ExprNode::SelfRef) {
        args.iter()
            .enumerate()
            .map(|(i, a)| coerce_arg_for_class_method(method, i, a))
            .collect()
    } else if let ExprNode::Const { path } = &*r.node {
        let class = path.last().map(|s| s.as_str()).unwrap_or("");
        // Try the hand-written runtime sigs first (Db, Broadcasts),
        // then fall back to the current class's method table. The
        // latter covers `Self::new(...)` / `Class::method(...)` shapes
        // where path.last() matches the class currently being
        // emitted — `Article::new()` from inside `impl Article`.
        let param_tys = external_class_method_param_tys(class, method)
            .or_else(|| current_class_method_param_tys(method));
        if let Some(param_tys) = param_tys {
            let mut out: Vec<String> = Vec::with_capacity(param_tys.len().max(args.len()));
            for (i, _) in param_tys.iter().enumerate() {
                match (args.get(i), param_tys.get(i)) {
                    // Caller-supplied arg: apply per-param coercion.
                    (Some(a), Some(pt)) => out.push(coerce_arg_for_param_ty(a, pt)),
                    // Missing trailing arg: synthesize a default for
                    // shapes that have one (Hash → HashMap::new(),
                    // etc.). Ruby `def initialize(attrs = {})`
                    // accepts zero-arg `Article.new`; Rust needs the
                    // explicit default. Without this `Class::new()`
                    // sites trip E0061.
                    (None, Some(pt)) => match synth_default_for_ty(pt) {
                        Some(d) => out.push(d),
                        None => break,
                    },
                    _ => break,
                }
            }
            // If caller passed MORE args than callee declares (rare —
            // splat / overload patterns), append the extras un-coerced.
            for a in args.iter().skip(param_tys.len()) {
                out.push(emit_expr(a));
            }
            out
        } else {
            args_s
        }
    } else if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Class { .. }))
        && method.ends_with('=')
        && args.len() == 1
    {
        // Setter convention coercion for instance-Send recvs whose Ty
        // is a known class: `instance.set_<col>(value)` came from the
        // model lowerer's `attr_writer` shim, where the setter param
        // ty equals the column ty. rust2 emits `Ty::Str/Sym` params
        // as `&str`, but the lowerer hands String-shaped args at
        // sites like `instance.set_body(row.body())` (where
        // `row.body()` returns owned `String`). Wrap owned-String
        // sources with `&(...)` so the borrow matches. Non-Str
        // setter args (`set_id(i64)`) pass through unchanged.
        //
        // Heuristic — the Const-recv arm above uses an explicit
        // param-Tys table; for instance-Sends we don't have a global
        // sibling registry yet, so the setter-name + arg-Ty + owned-
        // node combo carries the same signal. Limited to one-arg
        // calls because that's the AR `set_<col>` shape; broader
        // setter shapes (multi-arg) can opt in later.
        let mut out: Vec<String> = Vec::with_capacity(1);
        let coerced = coerce_arg_for_param_ty(
            &args[0],
            // Use the arg's body-typer Ty as the param Ty: setter
            // params for Str cols are typed Str, matching the row
            // accessor's return Ty. For non-Str args the coerce
            // function returns the bare emit.
            args[0].ty.as_ref().unwrap_or(&crate::ty::Ty::Untyped),
        );
        out.push(coerced);
        out
    } else {
        args_s
    };
    let recv_s = emit_expr(r);
    // Static method dispatch — `Type.method(args)` in Ruby becomes
    // `Type::method(args)` in Rust when the receiver is a Const
    // (class/module reference). The `.` form binds to a value
    // receiver; `::` binds to a type.
    let dispatch = if matches!(&*r.node, ExprNode::Const { .. }) {
        "::"
    } else {
        "."
    };
    if final_args.is_empty() {
        format!("{recv_s}{dispatch}{rewritten_method}()")
    } else {
        format!("{recv_s}{dispatch}{rewritten_method}({})", final_args.join(", "))
    }
}

/// Apply callee-back-propagation coercion for a single arg in a
/// class-/instance-method call where the callee is in
/// `CLASS_METHOD_PARAM_TYS`. Covers three coercion families:
///
/// 1. **HashMap shape transform**: callee `Hash<_, Untyped>` with arg
///    `Hash<_, *>` of differing K/V → wrap with `.into_iter().map().
///    collect()` into `HashMap<String, Value>`.
/// 2. **Value → primitive**: callee `Str|Int|Bool|Float` with arg
///    typed Untyped (Value) → `.as_X().unwrap()` via
///    `value_narrowing_coercion`. Closes the `self.set_id(row["id"])`
///    shape in lowered model `assign_from_row` / `new` bodies.
/// 3. **String → &str**: callee `Str|Sym` (rust2 emits `&str` for
///    these param positions) with arg typed `Str|Sym` from a
///    non-literal source (Var/Send/Ivar — produces owned String) →
///    `&(arg)`. The body-typer's view + str_color may already
///    handle some of these; the call-site path catches the rest.
fn coerce_arg_for_class_method(method: &str, idx: usize, arg: &Expr) -> String {
    let Some(param_ty) = class_method_param_ty(method, idx) else {
        return emit_expr(arg);
    };
    coerce_arg_for_param_ty(arg, &param_ty)
}

/// Core callee-back-propagation coercion: given an arg's `Expr` and
/// the callee's declared param `Ty`, return the emit string with the
/// appropriate coercion applied. Three families:
///
/// 1. **HashMap shape transform**: callee `Hash<_, Untyped>` with arg
///    `Hash<_, *>` of differing K/V → wrap with `.into_iter().map().
///    collect()` into `HashMap<String, Value>`.
/// 2. **Value → primitive**: callee `Str|Int|Bool|Float` with arg's
///    body-typer Ty (post-Nil peel) `Untyped` (Value) → append
///    `.as_X().unwrap()` via `value_narrowing_coercion`.
/// 3. **String → &str (Borrow)**: callee `Str|Sym` (rust2 emits
///    `&str` for these param positions) with arg from a non-literal
///    String-producing source (Var/Send/Ivar) → `&(raw)`.
fn coerce_arg_for_param_ty(arg: &Expr, param_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let raw = emit_expr(arg);
    let arg_ty_peeled = arg.ty.as_ref().map(peel_nil);

    if let Ty::Hash { value: pv, .. } = param_ty {
        if matches!(pv.as_ref(), Ty::Untyped) {
            // Var arg with a local Hash type that doesn't match —
            // wrap with the K/V-coercing conversion.
            if let Some((_lk, _lv)) = arg_hash_var_local_ty(arg) {
                return format!(
                    "{raw}.into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v))).collect::<std::collections::HashMap<String, serde_json::Value>>()"
                );
            }
            // Hash-literal arg: HashMap::from([…]) typically infers
            // `HashMap<&str, T>` from the first entry, which won't
            // unify with the callee's `HashMap<String, Value>` even
            // when each entry's value is String/Int/Bool. Apply the
            // same transform unconditionally — the conversion takes
            // any IntoIterator<Item = (impl Into<String>, impl
            // Into<Value>)> through ours.
            if matches!(&*arg.node, ExprNode::Hash { .. }) {
                return format!(
                    "{raw}.into_iter().map(|(k, v)| (k.to_string(), serde_json::Value::from(v))).collect::<std::collections::HashMap<String, serde_json::Value>>()"
                );
            }
        }
    }

    if matches!(arg_ty_peeled, Some(Ty::Untyped)) {
        if let Some(coerce) = value_narrowing_coercion(param_ty) {
            return format!("({raw}).{coerce}");
        }
    }

    // Family 4: primitive → Value. Callee/target slot wants
    // `serde_json::Value` (`Untyped`), arg is a concrete primitive
    // (Str/Sym/Int/Float/Bool) — wrap with `Value::from(...)`.
    // Closes the `attributes` return shape where each model field
    // (String/i64/Bool) needs to land in HashMap<String, Value>;
    // also fires inside tail-position HashMap literals via
    // emit_hash's return-type-driven coercion.
    //
    // `Value::from` takes by value; for Ivar reads of non-Copy
    // fields (`self.body` typed `String`), the raw is `self.body`
    // which is `&String` (we're inside `&self`/`&mut self`).
    // Wrapping moves out of the shared borrow (E0507). Clone the
    // field first to materialize the owned value.
    if matches!(param_ty, Ty::Untyped)
        && matches!(
            arg_ty_peeled,
            Some(Ty::Str | Ty::Sym | Ty::Int | Ty::Float | Ty::Bool)
        )
    {
        let needs_clone = matches!(&*arg.node, ExprNode::Ivar { .. })
            && !matches!(arg_ty_peeled, Some(Ty::Int | Ty::Float | Ty::Bool));
        if needs_clone {
            return format!("serde_json::Value::from({raw}.clone())");
        }
        return format!("serde_json::Value::from({raw})");
    }

    if matches!(param_ty, Ty::Str | Ty::Sym) && arg.str_coercion.is_none() {
        // Peek through `Cast` wrappers — the model lowerer wraps row
        // accessors in `Cast { Send(row.col), col_ty }` to bridge
        // Crystal's nilable row holder, but rust2's row class is
        // already non-Nilable so the Cast renders as the bare inner
        // call. The "is this owned String?" check has to see the
        // inner node to fire.
        let owned_producing_node = |n: &ExprNode| {
            matches!(
                n,
                ExprNode::Var { .. } | ExprNode::Send { .. } | ExprNode::Ivar { .. }
            )
        };
        let arg_is_owned = matches!(arg_ty_peeled, Some(Ty::Str | Ty::Sym))
            && (owned_producing_node(&*arg.node)
                || matches!(
                    &*arg.node,
                    ExprNode::Cast { value, .. } if owned_producing_node(&*value.node)
                ));
        if arg_is_owned {
            return format!("&({raw})");
        }
    }

    raw
}

/// When a Cast's source type renders as `serde_json::Value` at the
/// rust2 emit (a non-Nilable multi-variant Union — `Union<i64,
/// String, …>` from the lowerer-synthesized column-union types), and
/// the target type is a primitive (`Str`/`Sym`/`Int`/`Float`/
/// `Bool`), emit the corresponding `.as_X().unwrap()` coercion.
/// Without this, `value.as(i64)` would emit as bare `value`
/// against an `i64`-typed Ivar field — `set_index` arm bodies trip
/// E0308.
fn cast_via_value_for_union(value: &Expr, target_ty: &crate::ty::Ty) -> Option<String> {
    use crate::ty::Ty;
    // Check if the source's rust2-rendered Ty would be `Value`. The
    // ty.rs Union arm falls back to `serde_json::Value` for any
    // non-Nilable multi-variant Union (single Option<T> renders as
    // `Option<T>`, not Value).
    let value_shaped = match value.ty.as_ref() {
        Some(Ty::Union { variants }) => {
            let has_nil = variants.iter().any(|v| matches!(v, Ty::Nil));
            !(variants.len() == 2 && has_nil)
        }
        _ => false,
    };
    if !value_shaped {
        return None;
    }
    let raw = emit_expr(value);
    match target_ty {
        Ty::Str | Ty::Sym => {
            Some(format!("({raw}).as_str().unwrap().to_string()"))
        }
        Ty::Int => Some(format!("({raw}).as_i64().unwrap()")),
        Ty::Float => Some(format!("({raw}).as_f64().unwrap()")),
        Ty::Bool => Some(format!("({raw}).as_bool().unwrap()")),
        _ => None,
    }
}

/// Field-position coercion: variant of `coerce_arg_for_param_ty`
/// for the constructor's `let <field> = <value>` rewrite. Two
/// differences from param-position coercion:
///
/// 1. **String fields want owned `String`**, not `&str`. Param
///    positions accept `&str` (rust2 emits `Str/Sym` params as
///    `&str`) but struct fields store owned `String`. After the
///    Value→`&str` `.as_str().unwrap()`, append `.to_string()`.
/// 2. **Union-containing-Untyped triggers Value-narrowing too** —
///    `BoolOp::Or` of `hash[k] || 0` (Option<Value> || Int) gets
///    body-typed `Union<Union<Untyped, Nil>, Int>`, neither peel_nil
///    nor a flat Union-of-Untyped+Nil. Recursively probe for
///    Untyped in the Ty tree so the Value-shaped emit gets coerced.
fn coerce_arg_for_field_ty(arg: &Expr, field_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let raw = emit_expr(arg);
    let value_shaped = arg.ty.as_ref().map(ty_contains_untyped).unwrap_or(false);
    if value_shaped {
        let coercion = match field_ty {
            Ty::Str | Ty::Sym => Some("as_str().unwrap().to_string()"),
            Ty::Int => Some("as_i64().unwrap()"),
            Ty::Float => Some("as_f64().unwrap()"),
            Ty::Bool => Some("as_bool().unwrap()"),
            _ => None,
        };
        if let Some(c) = coercion {
            return format!("({raw}).{c}");
        }
    }
    raw
}

/// Recursively check whether `ty` contains `Untyped` anywhere — at
/// the top level or nested inside any `Union`. Used by
/// `coerce_arg_for_field_ty` to recognize Value-shaped expressions
/// whose body-typer-recorded `Ty` is a multi-layer Union (the
/// `BoolOp::Or` pattern `hash[k] || default` types as
/// `Union<Union<Untyped, Nil>, T>`, which `peel_nil` doesn't strip).
///
/// Also treats non-Option multi-variant Unions as Value-shaped:
/// rust2's `ty::rust_ty` renders these as `serde_json::Value` (see
/// `option_shape`-unwrap fallback). So a `Union<i64, String, …>`
/// from the lowerer-synthesized column-union types behaves like
/// Value at the emit boundary.
fn ty_contains_untyped(ty: &crate::ty::Ty) -> bool {
    use crate::ty::Ty;
    match ty {
        Ty::Untyped => true,
        Ty::Union { variants } => variants.iter().any(ty_contains_untyped),
        _ => false,
    }
}

/// Per-method positional-param Tys for hand-written runtime modules
/// (`Db`, future `Broadcasts`, etc.) that aren't surfaced through
/// `CLASS_METHOD_PARAM_TYS`. The Const-recv dispatch in emit_send
/// consults this so `Db::prepare(format!(...))` (String arg, `&str`
/// param) inserts the Borrow coercion automatically — same pattern
/// the Self::method path applies for in-class callees.
fn external_class_method_param_tys(class: &str, method: &str) -> Option<Vec<crate::ty::Ty>> {
    use crate::ty::Ty;
    let hash_str_untyped = || Ty::Hash {
        key: Box::new(Ty::Str),
        value: Box::new(Ty::Untyped),
    };
    match (class, method) {
        ("Db", "prepare") => Some(vec![Ty::Str]),
        ("Db", "exec") => Some(vec![Ty::Str]),
        ("Db", "step") => Some(vec![Ty::Int]),
        ("Db", "column_int") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_text") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "column_bool") => Some(vec![Ty::Int, Ty::Int]),
        ("Db", "finalize") => Some(vec![Ty::Int]),
        ("Db", "escape_string") => Some(vec![Ty::Str]),
        ("Db", "escape_int") => Some(vec![Ty::Int]),
        ("Db", "escape_bool") => Some(vec![Ty::Bool]),
        ("Db", "last_insert_rowid") => Some(vec![]),
        // `Broadcasts::method(HashMap<String, Value>)` — the lowerer
        // emits kwargs as a HashMap; the runtime shim accepts that
        // shape and pulls named fields out (see
        // `runtime/rust/broadcasts.rs::record`).
        ("Broadcasts", "append" | "prepend" | "replace" | "remove") => {
            Some(vec![hash_str_untyped()])
        }
        _ => None,
    }
}

/// Recv-Ty-aware method bridges — Ruby method calls whose Rust
/// analog differs by receiver type. Mirrors the structure of
/// `typescript/expr.rs`'s recv-ty match block (around lines
/// 1972–2245). Returns `Some(emitted)` when a bridge fires; `None`
/// when the recv ty / method combo isn't handled here and should
/// fall through to the generic dispatch path.
///
/// Predicates retain their trailing `?` at this level — the
/// generic `rewrite_method_name` does the suffix strip later, but
/// the Ty-aware bridges match on the Ruby-shape names directly.
///
/// Where Ruby methods return Integer (`.length`, `.size`, `.count`),
/// the bridge inserts `(... as i64)` so downstream arithmetic /
/// comparisons against Ruby loop counters (`let mut i = 0_i64`)
/// compile without per-callsite widens.
fn dispatch_method_by_recv_ty(
    recv: &Expr,
    method: &str,
    args: &[Expr],
) -> Option<String> {
    use crate::ty::Ty;
    let recv_s = emit_expr(recv);
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    // Peel `Union<T, Nil>` to `T` for dispatch. The body-typer reports
    // Ruby's nil-on-miss shape for `arr[i]` / `hash[k]` / `first`/`last`
    // (returns `T | Nil`), but rust2 emits these as panic-on-miss
    // (`arr[i]`, `hash[&k]`), so the runtime value really is `T`.
    // Matching `Some(Ty::Str)` directly would miss every `let x = arr[i]`
    // binding, since `x`'s recorded Ty is `Union<Str, Nil>`.
    let recv_ty = recv.ty.as_ref().map(peel_nil);
    match recv_ty {
        Some(Ty::Array { .. }) => match method {
            "size" | "length" | "count" if args.is_empty() => {
                Some(format!("({recv_s}.len() as i64)"))
            }
            "empty?" if args.is_empty() => Some(format!("{recv_s}.is_empty()")),
            "any?" if args.is_empty() => Some(format!("!{recv_s}.is_empty()")),
            // `first` / `last` on Vec return Option<&T>; `.cloned()`
            // gives Option<T> matching Ruby's nil-or-value semantics.
            "first" if args.is_empty() => Some(format!("{recv_s}.first().cloned()")),
            "last" if args.is_empty() => Some(format!("{recv_s}.last().cloned()")),
            // `to_a` on an Array is the identity in Ruby.
            "to_a" if args.is_empty() => Some(recv_s.clone()),
            // `reverse` / `sort` return new arrays in Ruby. Clone-
            // then-mutate keeps the receiver intact and produces an
            // owned Vec the caller can pass on.
            "reverse" if args.is_empty() => Some(format!(
                "{{ let mut __v = {recv_s}.clone(); __v.reverse(); __v }}"
            )),
            "sort" if args.is_empty() => Some(format!(
                "{{ let mut __v = {recv_s}.clone(); __v.sort(); __v }}"
            )),
            // `join` — Ruby's no-arg default is `$,` (typically nil
            // → ""); explicit `""` matches that. Single-arg form
            // delegates to the Rust `Vec::join` which accepts &str.
            "join" if args.is_empty() => Some(format!("{recv_s}.join(\"\")")),
            "join" if args.len() == 1 => {
                Some(format!("{recv_s}.join({})", args_s[0]))
            }
            // `arr.include?(x)` — Ruby's Array membership test. Rust's
            // `Vec::contains` takes `&T`, so `cols.contains("k")` fails
            // when `cols: Vec<String>` (the literal is `&str`, not
            // `&String`). `iter().any(|c| c == arg)` sidesteps the
            // Borrow constraint and works for any element type with
            // a `PartialEq` impl against the arg expression.
            "include?" | "contains?" if args.len() == 1 => Some(format!(
                "{recv_s}.iter().any(|__c| __c == {})",
                args_s[0]
            )),
            _ => None,
        },
        Some(Ty::Str) | Some(Ty::Sym) => match method {
            "empty?" if args.is_empty() => Some(format!("{recv_s}.is_empty()")),
            "size" | "length" if args.is_empty() => {
                Some(format!("({recv_s}.len() as i64)"))
            }
            "upcase" if args.is_empty() => Some(format!("{recv_s}.to_uppercase()")),
            "downcase" if args.is_empty() => Some(format!("{recv_s}.to_lowercase()")),
            // `strip` → `trim()` returns &str; `.to_string()` forces
            // owned to match Ruby's `String#strip` return shape.
            "strip" if args.is_empty() => {
                Some(format!("{recv_s}.trim().to_string()"))
            }
            // `reverse` on String — Rust has no direct method;
            // `.chars().rev().collect::<String>()` matches Ruby's
            // codepoint-reversal semantics.
            "reverse" if args.is_empty() => Some(format!(
                "{recv_s}.chars().rev().collect::<String>()"
            )),
            // `chars` returns Array<String> in Ruby; mirror with
            // `Vec<String>` (each char converted to a one-char String).
            "chars" if args.is_empty() => Some(format!(
                "{recv_s}.chars().map(|c| c.to_string()).collect::<Vec<String>>()"
            )),
            "start_with?" if args.len() == 1 => {
                Some(format!("{recv_s}.starts_with({})", args_s[0]))
            }
            "end_with?" if args.len() == 1 => {
                Some(format!("{recv_s}.ends_with({})", args_s[0]))
            }
            "include?" if args.len() == 1 => {
                Some(format!("{recv_s}.contains({})", args_s[0]))
            }
            _ => None,
        },
        Some(Ty::Hash { .. }) => match method {
            // `key?` / `has_key?` / `include?` all probe key
            // presence on a Hash. HashMap has `contains_key`.
            "key?" | "has_key?" | "include?" if args.len() == 1 => {
                Some(format!("{recv_s}.contains_key({})", args_s[0]))
            }
            "empty?" if args.is_empty() => Some(format!("{recv_s}.is_empty()")),
            "any?" if args.is_empty() => Some(format!("!{recv_s}.is_empty()")),
            "size" | "length" if args.is_empty() => {
                Some(format!("({recv_s}.len() as i64)"))
            }
            "keys" if args.is_empty() => Some(format!(
                "{recv_s}.keys().cloned().collect::<Vec<_>>()"
            )),
            "values" if args.is_empty() => Some(format!(
                "{recv_s}.values().cloned().collect::<Vec<_>>()"
            )),
            // `dup` / `clone` make a shallow copy. HashMap::clone()
            // matches both.
            "dup" | "clone" if args.is_empty() => Some(format!("{recv_s}.clone()")),
            // `hash.delete(k)` — Ruby removes by key, returns the
            // removed value (or nil). HashMap::remove takes `&Q where
            // K: Borrow<Q>` and returns `Option<V>`. Emit-side only;
            // user-defined classes with their own `delete(...)` method
            // (e.g. `ActiveRecordAdapter::delete(table, id)`) bypass
            // this arm because the recv-Ty match fails.
            //
            // The `&` prefix is needed when the arg emits as `String`
            // (owned) but skipped when it's already `&str` (a literal
            // or borrowed). For HashMap<String, _>, `.remove("k")`
            // works (`&str` borrows from `str`); `.remove(&"k")`
            // fails (`&&str`, String doesn't Borrow<&str>).
            "delete" if args.len() == 1 => {
                let key_emit = if matches!(
                    &*args[0].node,
                    ExprNode::Lit { value: Literal::Str { .. } | Literal::Sym { .. } }
                ) {
                    args_s[0].clone()
                } else {
                    format!("&{}", args_s[0])
                };
                Some(format!("{recv_s}.remove({key_emit})"))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Peel `Union<T, Nil>` to `T` for dispatch-time matching. Returns the
/// original Ty unchanged if it isn't a 2-variant `T | Nil` union.
/// Mirrors `analyze::body::peel_nilable` — kept locally so emit doesn't
/// reach across into a private analyzer helper.
fn peel_nil(ty: &crate::ty::Ty) -> &crate::ty::Ty {
    use crate::ty::Ty;
    if let Ty::Union { variants } = ty {
        if variants.len() == 2 {
            if let Some(idx) = variants.iter().position(|v| matches!(v, Ty::Nil)) {
                return &variants[1 - idx];
            }
        }
    }
    ty
}

/// Ruby method names → Rust analog. Generic (recv-type-agnostic)
/// table; a richer pass keyed on the receiver's `Ty` can layer on
/// later when ambiguities show up in real emit. The `?` / `!` strip
/// is the universal predicate sanitization — Rust idents reject
/// those suffixes, and the framework Ruby leans on Ruby's predicate
/// naming conventions heavily (`empty?`, `is_a?`, `nil?`, `key?`).
fn rewrite_method_name(m: &str) -> String {
    let bridged = match m {
        "to_s" => "to_string",
        "length" => "len",
        "nil?" => "is_none",
        "empty?" => "is_empty",
        "key?" => "contains_key",
        "has_key?" => "contains_key",
        "include?" => "contains",
        // `delete` is NOT blanket-rewritten: Ruby has it on Hash (remove
        // by key) AND on user-defined classes (the `ActiveRecordAdapter`
        // trait's `delete(table, id)` is the visible case). The Hash
        // case is handled in `dispatch_method_by_recv_ty`; other
        // receivers keep the Ruby name and resolve through their own
        // method definitions.
        other => other,
    };
    sanitize_ident(bridged)
}

/// Sanitize a Ruby identifier for Rust:
/// * `foo!` (bang form, conventionally Ruby's "raises on failure")
///   → `foo_bang`. Preserves the distinction vs the non-bang sibling
///   (`def create` vs `def create!` both exist on AR::Base; stripping
///   the `!` would collide them). Not idiomatic Rust (the canonical
///   form would be `try_foo` Result vs `foo` panic), but mechanical
///   and unambiguous.
/// * `foo?` (predicate) → `foo`. Ruby's question-mark convention has
///   no Rust analog; the body just returns `bool` either way.
/// * `foo=` (setter, synthesized by `attr_writer` / `attr_accessor`)
///   → `set_foo`. Rust has no setter syntax; explicit-named methods
///   are the convention.
/// * Reserved Rust keywords → `r#keyword` raw-identifier form.
///
/// Public so `method.rs` can use the same rule at `pub fn`
/// definition sites — defines and call sites share the transform
/// so name agreement holds across both.
/// True when `outer` is a `Union<inner, Nil>` (Option-shape) and
/// `inner` matches the unwrapped type. Used by the Var-emit narrowing
/// write-back to detect when the body-typer's narrowed type is the
/// `Option<T>` → `T` form (the common `unless x.nil?` pattern).
/// Given a narrowed Ty inside an `is_a?(Class)` branch on a
/// `serde_json::Value`-typed binding, return the `.as_X().unwrap()`
/// (or similar) coercion shape that extracts the inner value, or
/// None if the narrowed Ty doesn't map to a Value accessor.
fn value_narrowing_coercion(narrowed: &crate::ty::Ty) -> Option<&'static str> {
    match narrowed {
        crate::ty::Ty::Str => Some("as_str().unwrap()"),
        crate::ty::Ty::Bool => Some("as_bool().unwrap()"),
        crate::ty::Ty::Int => Some("as_i64().unwrap()"),
        crate::ty::Ty::Float => Some("as_f64().unwrap()"),
        _ => None,
    }
}

/// True if `ty` is `Option<T>` shape — a `Union { variants }` containing
/// exactly two variants, one of which is `Nil`. Mirrors rust2's
/// Option emit convention; the `is_option_of` variant adds a check
/// that the non-Nil variant matches a specific inner type.
fn is_option_ty(ty: &crate::ty::Ty) -> bool {
    matches!(
        ty,
        crate::ty::Ty::Union { variants }
            if variants.len() == 2 && variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
    )
}

fn is_option_of(outer: &crate::ty::Ty, inner: &crate::ty::Ty) -> bool {
    let crate::ty::Ty::Union { variants } = outer else {
        return false;
    };
    if variants.len() != 2 {
        return false;
    }
    let has_nil = variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil));
    let other = variants
        .iter()
        .find(|v| !matches!(v, crate::ty::Ty::Nil));
    matches!(other, Some(o) if has_nil && o == inner)
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

/// Built-in container classes whose `[]` / `[]=` should stay as the
/// Rust bracket-index syntax (`HashMap`, `Vec`, etc. via Index/IndexMut).
/// User-defined classes route through the `get_index` / `set_index`
/// method rewrite instead.
pub(super) fn is_builtin_container_class(name: &str) -> bool {
    let last = name.rsplit("::").next().unwrap_or(name);
    matches!(
        last,
        "Hash" | "HashWithIndifferentAccess" | "Array" | "String"
            | "Flash" | "Session"
            | "Parameters"
            | "Errors" | "ErrorCollection"
    )
}

/// Wrap an emitted RHS with `serde_json::Value::from(...)` when the
/// expression's Ty isn't already `serde_json::Value`. Used by the
/// `set_index` call-site emit (Ty::Class indexer dispatch) — the
/// `def []=(_, untyped)` signature renders the value param as Value,
/// so a String/Int/Bool RHS needs explicit conversion.
pub(super) fn coerce_to_value(value: &Expr, rhs: &str) -> String {
    use crate::ty::Ty;
    let already_value = matches!(
        value.ty.as_ref(),
        Some(Ty::Untyped)
            | Some(Ty::Var { .. })
            | Some(Ty::Record { .. })
            | Some(Ty::Hash { .. })
    );
    if already_value {
        rhs.to_string()
    } else {
        format!("serde_json::Value::from({rhs})")
    }
}

pub(super) fn sanitize_ident(name: &str) -> String {
    // Operator-method names from Ruby don't translate to Rust syntax;
    // emit them as descriptive Rust identifiers. Call sites are
    // rewritten to match in `emit_assign` (LValue::Index) and the
    // Send-`[]`/`[]=` paths in `emit_send_method`.
    if name == "[]" {
        return "get_index".to_string();
    }
    if name == "[]=" {
        return "set_index".to_string();
    }
    let s = if let Some(base) = name.strip_suffix('!') {
        // `bang!` collides with the non-bang sibling after `?`-strip,
        // so suffix with `_bang` rather than dropping the marker.
        return format!("{base}_bang");
    } else if let Some(base) = name.strip_suffix('=') {
        // `foo=` becomes `set_foo`. The `=` suffix is Ruby's setter
        // convention; Rust uses explicit-method-named setters.
        return format!("set_{base}");
    } else if let Some(base) = name.strip_suffix('?') {
        base
    } else {
        name
    };
    if is_rust_keyword(s) {
        format!("r#{s}")
    } else {
        s.to_string()
    }
}

/// Rust 2024 reserved-word set. The `r#ident` raw-identifier form
/// lifts the keyword restriction so user-defined names like `match`,
/// `loop`, `type` can become function/struct names. Matches the
/// `rustc_lexer` keyword list; `r#` doesn't apply to a small group
/// of contextual keywords (`crate`, `self`, `Self`, `super`,
/// `extern`) but those are unlikely in a Ruby source surface.
fn is_rust_keyword(name: &str) -> bool {
    matches!(
        name,
        "as" | "break" | "const" | "continue" | "crate" | "else" | "enum"
            | "extern" | "false" | "fn" | "for" | "if" | "impl" | "in"
            | "let" | "loop" | "match" | "mod" | "move" | "mut" | "pub"
            | "ref" | "return" | "self" | "Self" | "static" | "struct"
            | "trait" | "true" | "type" | "unsafe" | "use" | "where"
            | "while" | "async" | "await" | "dyn"
            | "abstract" | "become" | "box" | "do" | "final" | "macro"
            | "override" | "priv" | "typeof" | "unsized" | "virtual"
            | "yield" | "try"
    )
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    // Rust `format!` macro is the natural interp target.
    // Lift literal text into the format string (escaping `{`/`}`),
    // each `#{expr}` becomes a `{}` placeholder + an arg.
    let mut fmt = String::from("format!(\"");
    let mut args: Vec<String> = Vec::new();
    for p in parts {
        match p {
            InterpPart::Text { value } => {
                for c in value.chars() {
                    match c {
                        '"' => fmt.push_str("\\\""),
                        '\\' => fmt.push_str("\\\\"),
                        '\n' => fmt.push_str("\\n"),
                        '\r' => fmt.push_str("\\r"),
                        '\t' => fmt.push_str("\\t"),
                        '{' => fmt.push_str("{{"),
                        '}' => fmt.push_str("}}"),
                        other => fmt.push(other),
                    }
                }
            }
            InterpPart::Expr { expr } => {
                fmt.push_str("{}");
                args.push(emit_expr(expr));
            }
        }
    }
    fmt.push_str("\"");
    if !args.is_empty() {
        fmt.push_str(", ");
        fmt.push_str(&args.join(", "));
    }
    fmt.push(')');
    fmt
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "None".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => format!("{value}_i64"),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, .. } => format!("/* TODO rust2: Regex({pattern:?}) */"),
    }
}
