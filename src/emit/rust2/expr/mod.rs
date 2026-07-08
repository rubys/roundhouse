//! `rust2` expression emit — `Expr` IR → Rust source-text.
//!
//! Phase 2.1 scope: minimal handling for the inflector body shape
//! (Lit, Var, Send `==`, StringInterp, If). Extended file-by-file
//! through Phase 2 as each runtime file forces new IR shapes.

use crate::expr::{Expr, ExprNode, InterpPart, LValue};

mod assign;
mod control;
pub(crate) mod literal;
mod send;
pub(crate) mod util;
use assign::emit_assign;
use control::{emit_bool_op, emit_case, emit_if, emit_return, emit_seq, emit_while};
use literal::{attach_block, emit_array, emit_closure, emit_hash, emit_string_interp};
pub(super) use literal::emit_literal;
use send::{cast_via_value_for_union, coerce_arg_for_field_ty, emit_send};
pub(super) use send::coerce_arg_for_param_ty;
pub(super) use util::sanitize_ident;
use util::{indent, is_copy_ty, is_option_of, peel_nil, value_narrowing_coercion};

thread_local! {
    /// Pipeline-global emit context — install slot for `with_emit_ctx`.
    /// Holds `None` outside the install scope; `Some(Rc<EmitCtx>)`
    /// inside. All other emit state (per-class, per-method, transient
    /// render flags) lives as fields on the `EmitCtx` and is reachable
    /// via `current_emit_ctx()`.
    ///
    /// This is the only thread-local in rust2 emit after #24 Phases
    /// 1–4. It cannot itself move onto `EmitCtx` because it's the
    /// install point that publishes the `EmitCtx` to the rest of the
    /// emit code.
    static EMIT_CTX: std::cell::RefCell<Option<std::rc::Rc<super::EmitCtx>>> =
        std::cell::RefCell::new(None);
}

pub(super) fn with_param_types<F, R>(types: std::collections::HashMap<String, crate::ty::Ty>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_param_types called outside with_emit_ctx");
    let prev = std::mem::replace(&mut *ctx.param_types.borrow_mut(), types);
    let r = f();
    *ctx.param_types.borrow_mut() = prev;
    r
}

pub(super) fn param_ty(name: &str) -> Option<crate::ty::Ty> {
    current_emit_ctx().and_then(|ctx| ctx.param_types.borrow().get(name).cloned())
}

fn is_rebound_var(name: &str) -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.rebound_vars.borrow().contains(name))
        .unwrap_or(false)
}

pub(super) fn mark_rebound_var(name: &str) {
    let ctx = current_emit_ctx().expect("mark_rebound_var called outside with_emit_ctx");
    ctx.rebound_vars.borrow_mut().insert(name.to_string());
}

pub(super) fn local_var_ty(name: &str) -> Option<crate::ty::Ty> {
    current_emit_ctx().and_then(|ctx| ctx.local_var_types.borrow().get(name).cloned())
}

pub(super) fn mark_local_var_ty(name: &str, ty: crate::ty::Ty) {
    let ctx = current_emit_ctx().expect("mark_local_var_ty called outside with_emit_ctx");
    ctx.local_var_types.borrow_mut().insert(name.to_string(), ty);
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

/// Shared narrowing-write-back: when the body-typer narrowed `name`
/// from its declared `Option<T>` to `T` (or from `Untyped`→`T` via
/// `is_a?`), produce the unwrap-shape that exposes the narrowed runtime
/// value to downstream coercion. `narrowed_ty` is the Expr's `.ty`
/// (post-narrowing) at the use site; `name` is the binding identifier.
///
/// Returns `None` when no narrowing transformation applies, leaving the
/// caller to emit the bare identifier.
///
/// Called from both `ExprNode::Var` reads and the bareword param-read
/// shortcut in `emit_send` (`Send { recv: None, method, args: [] }`
/// where the lowerer emits implicit-self param references in view
/// partials).
pub(super) fn narrowed_param_read(
    name: &str,
    narrowed_ty: Option<&crate::ty::Ty>,
) -> Option<String> {
    let narrowed = narrowed_ty?;
    let declared = var_decl_ty(name)?;
    if is_option_of(&declared, narrowed) && !is_rebound_var(name) {
        return Some(format!("{name}.clone().unwrap()"));
    }
    if matches!(declared, crate::ty::Ty::Untyped) {
        if let Some(coerce) = value_narrowing_coercion(narrowed) {
            return Some(format!("{name}.{coerce}"));
        }
    }
    None
}

/// Snapshot the current REBOUND_VARS + LOCAL_VAR_TYPES, run `f`, then
/// restore the snapshot. Used by Seq emit to scope let-Some rebinds
/// and local declarations to the current Seq — nested blocks shouldn't
/// leak their bindings outward, and the surrounding emit shouldn't see
/// declarations from a child Seq.
pub(super) fn with_rebound_vars_scope<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_rebound_vars_scope called outside with_emit_ctx");
    let prev_rebound = ctx.rebound_vars.borrow().clone();
    let prev_locals = ctx.local_var_types.borrow().clone();
    let r = f();
    *ctx.rebound_vars.borrow_mut() = prev_rebound;
    *ctx.local_var_types.borrow_mut() = prev_locals;
    r
}

pub(super) fn with_current_return_ty<F, R>(ty: Option<crate::ty::Ty>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_current_return_ty called outside with_emit_ctx");
    let prev = std::mem::replace(&mut *ctx.current_return_ty.borrow_mut(), ty);
    let r = f();
    *ctx.current_return_ty.borrow_mut() = prev;
    r
}

pub(super) fn current_return_is_option() -> bool {
    current_emit_ctx()
        .map(|ctx| {
            matches!(
                ctx.current_return_ty.borrow().as_ref(),
                Some(crate::ty::Ty::Union { variants }) if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
            )
        })
        .unwrap_or(false)
}

/// True when the enclosing function returns unit (`()` — declared
/// `-> void` in RBS, `Ty::Nil` in IR). The trailing `nil` of a void-
/// shaped Ruby method's body needs to emit as `()` (or nothing),
/// NOT as `None` (which is the Option::None constructor and would
/// produce an E0308 in a void function context).
pub(super) fn current_return_is_unit() -> bool {
    current_emit_ctx()
        .map(|ctx| matches!(ctx.current_return_ty.borrow().as_ref(), Some(crate::ty::Ty::Nil)))
        .unwrap_or(false)
}

/// Run `f` with the module-singleton emit mode active. Used by
/// `library.rs` when the class shape signals a Ruby
/// `class << self; ... end` (every method is a class method).
/// `thread_local` selects the request-scoped slot shape (per-ivar
/// `thread_local!` `RefCell<Option<T>>`) over the default global
/// `Mutex<Option<T>>` — see `library.rs::REQUEST_SCOPED_SINGLETONS`.
pub(super) fn with_module_singleton<F, R>(active: bool, thread_local: bool, f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_module_singleton called outside with_emit_ctx");
    let prev = ctx.in_module_singleton.replace(active);
    let prev_tl = ctx.module_singleton_thread_local.replace(thread_local);
    let r = f();
    ctx.in_module_singleton.set(prev);
    ctx.module_singleton_thread_local.set(prev_tl);
    r
}

pub(super) fn in_module_singleton() -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.in_module_singleton.get())
        .unwrap_or(false)
}

pub(super) fn module_singleton_thread_local() -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.module_singleton_thread_local.get())
        .unwrap_or(false)
}

/// Keyed read on a thread-local module-singleton Hash ivar
/// (`@slots[k]` / `@slots.fetch(k, ...)` inside a request-scoped
/// singleton like ViewHelpers). The generic Ivar-read emit clones the
/// whole map out of the slot before indexing — for the slot store
/// that means copying the entire rendered page body on every
/// `get_yield` (roundhouse#32). This form borrows in place and clones
/// only the looked-up value. Returns the rendered
/// `Option<V>`-producing expression, or `None` when the shape doesn't
/// match (caller falls through to the generic emit).
pub(super) fn module_singleton_hash_get(recv: &Expr, key_s: &str) -> Option<String> {
    if !in_module_singleton() || !module_singleton_thread_local() {
        return None;
    }
    let ExprNode::Ivar { name } = &*recv.node else { return None };
    let slot = module_singleton_slot_name(name.as_str());
    Some(format!(
        "{slot}.with(|__s| __s.borrow().as_ref().and_then(|__m| __m.get({key_s}).cloned()))"
    ))
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
/// names not in the ivar table. Reads through `EmitCtx::ivar_types`
/// (Phase 2 of #24); returns `None` if no `EmitCtx` is installed
/// (early decide-pass walks run outside `with_emit_ctx` and don't
/// touch ivars).
pub(super) fn ivar_field_ty(name: &str) -> Option<crate::ty::Ty> {
    current_emit_ctx().and_then(|ctx| ctx.ivar_types.borrow().get(name).cloned())
}

/// Run `f` with the supplied ivar→type table active. Used by
/// `library.rs` to scope each `impl` block's emit. Swaps
/// `EmitCtx::ivar_types` with save-restore (Phase 2 of #24).
/// Must be called inside `with_emit_ctx` — panics otherwise.
pub(super) fn with_ivar_types<F, R>(types: std::collections::HashMap<String, crate::ty::Ty>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_ivar_types called outside with_emit_ctx");
    let prev = std::mem::replace(&mut *ctx.ivar_types.borrow_mut(), types);
    let r = f();
    *ctx.ivar_types.borrow_mut() = prev;
    r
}

pub(super) fn with_constructor_mode<F, R>(fields: Vec<String>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_constructor_mode called outside with_emit_ctx");
    let prev_mode = ctx.in_constructor.replace(true);
    let prev_fields = std::mem::replace(&mut *ctx.constructor_fields.borrow_mut(), fields);
    let r = f();
    ctx.in_constructor.set(prev_mode);
    *ctx.constructor_fields.borrow_mut() = prev_fields;
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
    // The clone-on-multi-read decision is now in
    // `decide::last_use::stamp` — it stamps `CLONE_AT` per Var read
    // node, replacing the per-method `CLONE_VARS` thread-local set
    // that used to live here. See Stage 3 of #22.
    let ctx = current_emit_ctx().expect("with_method_scope called outside with_emit_ctx");
    let prev_mut = std::mem::replace(&mut *ctx.mut_vars.borrow_mut(), mut_vars);
    let prev_declared = std::mem::replace(
        &mut *ctx.declared_vars.borrow_mut(),
        std::collections::HashSet::new(),
    );
    let prev_back_prop = std::mem::replace(
        &mut *ctx.back_propagated_hash_locals.borrow_mut(),
        std::collections::HashSet::new(),
    );
    let r = f();
    *ctx.mut_vars.borrow_mut() = prev_mut;
    *ctx.declared_vars.borrow_mut() = prev_declared;
    *ctx.back_propagated_hash_locals.borrow_mut() = prev_back_prop;
    r
}

/// Emit a Send's *immediate* recv. When the recv is a Var (or a Send
/// shape that resolves to a bare param read — Ruby implicit-self), set
/// `SUPPRESS_VAR_CLONE` for the duration so the Var arm skips its
/// multi-read `.clone()` append. Auto-ref handles `&self`/`&mut self`
/// at recv positions; the explicit clone was breaking `&mut self`
/// setters (fixture loader `instance.set_id(...)`). Falls through to
/// plain `emit_expr` for non-Var recvs — Consts and sub-Sends manage
/// their own recv emission.
pub(super) fn emit_send_recv(r: &Expr) -> String {
    let is_bare_var = matches!(&*r.node, ExprNode::Var { .. });
    let s = if !is_bare_var {
        emit_expr(r)
    } else {
        let ctx = current_emit_ctx().expect("emit_send_recv called outside with_emit_ctx");
        let prev = ctx.suppress_var_clone.replace(true);
        let s = emit_expr(r);
        ctx.suppress_var_clone.set(prev);
        s
    };
    wrap_if_needs_parens(r, s)
}

/// Stage 1 (#22): consult the `NEEDS_PARENS` bit stamped by the
/// decide pass and wrap if set. Used by `emit_send_recv` and by
/// any other site that places an Expr's emit in a Rust *primary-
/// demanding* position (binary-op LHS, `as` cast LHS, unary
/// operand). The bit's semantics are "this Expr's emit shape is
/// non-primary, wrap it when chained as a recv-equivalent".
pub(super) fn wrap_if_needs_parens(e: &Expr, emitted: String) -> String {
    if e.decisions & super::decide::bits::NEEDS_PARENS != 0 {
        format!("({emitted})")
    } else {
        emitted
    }
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

pub(super) fn render_self_literal() -> String {
    let ctx = current_emit_ctx().expect("render_self_literal called outside with_emit_ctx");
    let fields = ctx.constructor_fields.borrow();
    if fields.is_empty() {
        "Self {}".to_string()
    } else {
        format!("Self {{ {} }}", fields.join(", "))
    }
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
    let ctx = current_emit_ctx().expect("with_static_methods called outside with_emit_ctx");
    let prev = std::mem::replace(&mut *ctx.static_methods.borrow_mut(), methods);
    let r = f();
    *ctx.static_methods.borrow_mut() = prev;
    r
}

/// Set the current class's method-name → positional-param-Tys
/// table for the duration of `f`. Used by `library.rs` to seed the
/// `Self::method(args)` arg-coercion lookup in emit_send. Swaps
/// `EmitCtx::class_method_param_tys` with save-restore (Phase 2 of
/// #24). Must be called inside `with_emit_ctx`.
pub(super) fn with_class_method_param_tys<F, R>(
    map: std::collections::HashMap<String, Vec<crate::ty::Ty>>,
    f: F,
) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_class_method_param_tys called outside with_emit_ctx");
    let prev = std::mem::replace(&mut *ctx.class_method_param_tys.borrow_mut(), map);
    let r = f();
    *ctx.class_method_param_tys.borrow_mut() = prev;
    r
}

/// Look up the current class's method param types by method name.
/// Returns None outside any class scope or when the method isn't
/// in the current class's table. Reads through
/// `EmitCtx::class_method_param_tys` (Phase 2 of #24).
pub(super) fn class_method_param_ty(method: &str, idx: usize) -> Option<crate::ty::Ty> {
    current_emit_ctx().and_then(|ctx| {
        ctx.class_method_param_tys
            .borrow()
            .get(method)
            .and_then(|tys| tys.get(idx).cloned())
    })
}

/// Return the full Vec of positional param Tys for a method in the
/// current class. Used by the Const-recv dispatch to check arity
/// + pad missing trailing args with defaults — Ruby's `def
/// initialize(attrs = {})` accepts zero-arg `Article.new`, but
/// Rust requires the explicit `HashMap::new()` default.
pub(super) fn current_class_method_param_tys(method: &str) -> Option<Vec<crate::ty::Ty>> {
    current_emit_ctx()
        .and_then(|ctx| ctx.class_method_param_tys.borrow().get(method).cloned())
}

/// Run `f` with the `EmitCtx` installed. Used by `rust2.rs::emit`
/// to wrap the per-file emit loop once the global registries are
/// built. Save-restore semantics: nested calls (rare — only the
/// outermost emit invocation today) cleanly stack.
pub(crate) fn with_emit_ctx<F, R>(ctx: super::EmitCtx, f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = std::rc::Rc::new(ctx);
    let prev = EMIT_CTX.with(|c| c.replace(Some(ctx)));
    let r = f();
    EMIT_CTX.with(|c| *c.borrow_mut() = prev);
    r
}

/// Borrow the currently-installed `EmitCtx`, if any. Used by
/// decide-pass walkers (#22 Stage 4) that run inside `with_emit_ctx`
/// and consult the global registries by reference rather than via
/// the per-accessor thread-local pattern. Returns `None` outside
/// the `with_emit_ctx` scope.
pub(crate) fn current_emit_ctx() -> Option<std::rc::Rc<super::EmitCtx>> {
    EMIT_CTX.with(|c| c.borrow().clone())
}

/// Per-position kwarg-default lookup for a Const-recv callee. Returns
/// the pre-rendered Rust literal for the param at `idx` when the
/// source-level default was a shape `render_param_default_literal`
/// recognized at collection time. None means either no default
/// existed, the index is out of range, or the registry doesn't have
/// this (class, method).
pub(super) fn global_class_method_param_default(
    class: &str,
    method: &str,
    idx: usize,
) -> Option<String> {
    EMIT_CTX.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|ctx| ctx.lookup_param_default(class, method, idx))
    })
}

/// Cross-class lookup: `(ClassName, method) → Vec<Ty>` for a callee
/// in a different LC than the currently-emitting class. Used by the
/// Const-recv dispatch as a fallback when the local
/// `current_class_method_param_tys` misses (the callee isn't a
/// sibling method on the same class). Returns the full positional-
/// param Ty list; emit_send pads missing trailing args via
/// `synth_default_for_ty`.
pub(super) fn global_class_method_param_tys(
    class: &str,
    method: &str,
) -> Option<Vec<crate::ty::Ty>> {
    EMIT_CTX.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|ctx| ctx.lookup_param_tys(class, method))
    })
}

/// Rich variant of `global_class_method_param_tys` returning the
/// full `Param` list (name + ty + kind). The kwargs-unpack pre-pass
/// in send-dispatch uses this to map a trailing-kwargs Hash literal
/// onto Keyword-param positions by name.
pub(super) fn global_class_method_params(
    class: &str,
    method: &str,
) -> Option<Vec<crate::ty::Param>> {
    EMIT_CTX.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|ctx| ctx.lookup_params(class, method))
    })
}

fn in_constructor() -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.in_constructor.get())
        .unwrap_or(false)
}

fn in_class_method() -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.in_class_method.get())
        .unwrap_or(false)
}

pub(super) fn with_class_method_scope<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_class_method_scope called outside with_emit_ctx");
    let prev = ctx.in_class_method.replace(true);
    let r = f();
    ctx.in_class_method.set(prev);
    r
}

pub(super) fn current_return_ty() -> Option<crate::ty::Ty> {
    current_emit_ctx().and_then(|ctx| ctx.current_return_ty.borrow().clone())
}

pub(super) fn is_declared_var(name: &str) -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.declared_vars.borrow().contains(name))
        .unwrap_or(false)
}

pub(super) fn declare_var(name: String) {
    let ctx = current_emit_ctx().expect("declare_var called outside with_emit_ctx");
    ctx.declared_vars.borrow_mut().insert(name);
}

pub(super) fn is_mut_var(name: &str) -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.mut_vars.borrow().contains(name))
        .unwrap_or(false)
}

/// Program-global: is `name` a `mutates_self`-flagged method on any
/// class? Populated by `collect_global_class_methods`. See
/// `EmitCtx::global_mutating_methods`.
pub(super) fn is_global_mutating_method(name: &str) -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.global_mutating_methods.contains(name))
        .unwrap_or(false)
}

/// True when an `each`-block body mutates its element — it calls a
/// `mutates_self`-flagged method on the block param. Canonical case is
/// the eager-load distribute loop's `a._preload_<assoc>(group)`. Such a
/// block must iterate the receiver with `iter_mut()` over the ORIGINAL
/// (no defensive `.clone()`); otherwise the `&mut self` writes land on
/// a throwaway clone and are silently dropped (roundhouse#40). Walks
/// the structural node kinds an `each` block uses; unrecognized shapes
/// return `false`, conservatively falling back to the cloning path.
fn each_block_mutates_param(body: &Expr, param: &str) -> bool {
    fn walk(e: &Expr, param: &str) -> bool {
        match &*e.node {
            ExprNode::Send { recv, method, args, block, .. } => {
                if let Some(r) = recv {
                    if matches!(&*r.node, ExprNode::Var { name, .. } if name.as_str() == param)
                        && is_global_mutating_method(method.as_str())
                    {
                        return true;
                    }
                    if walk(r, param) {
                        return true;
                    }
                }
                args.iter().any(|a| walk(a, param))
                    || block.as_ref().map(|b| walk(b, param)).unwrap_or(false)
            }
            ExprNode::Seq { exprs } => exprs.iter().any(|x| walk(x, param)),
            ExprNode::If { cond, then_branch, else_branch } => {
                walk(cond, param) || walk(then_branch, param) || walk(else_branch, param)
            }
            ExprNode::Assign { value, .. } | ExprNode::OpAssign { value, .. } => walk(value, param),
            ExprNode::Lambda { body, .. } => walk(body, param),
            ExprNode::Let { value, body, .. } => walk(value, param) || walk(body, param),
            ExprNode::Return { value } | ExprNode::Raise { value } | ExprNode::Splat { value } => {
                walk(value, param)
            }
            ExprNode::BoolOp { left, right, .. } => walk(left, param) || walk(right, param),
            ExprNode::While { cond, body, .. } => walk(cond, param) || walk(body, param),
            ExprNode::Array { elements, .. } => elements.iter().any(|x| walk(x, param)),
            ExprNode::Hash { entries, .. } => {
                entries.iter().any(|(k, v)| walk(k, param) || walk(v, param))
            }
            _ => false,
        }
    }
    walk(body, param)
}

pub(super) fn record_back_propagated_hash(name: String) {
    let ctx = current_emit_ctx().expect("record_back_propagated_hash called outside with_emit_ctx");
    ctx.back_propagated_hash_locals.borrow_mut().insert(name);
}

pub(super) fn is_back_propagated_hash(name: &str) -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.back_propagated_hash_locals.borrow().contains(name))
        .unwrap_or(false)
}

pub(super) fn in_return_tail() -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.in_return_tail.get())
        .unwrap_or(false)
}

/// Set the return-tail flag and run `f`. Used by `method.rs` around
/// the body emit of non-constructor instance methods, so the body's
/// top-level expression (or `Seq` tail / `Return` value) is recognized
/// as the function's return value.
pub(super) fn with_return_tail<F, R>(value: bool, f: F) -> R
where
    F: FnOnce() -> R,
{
    let ctx = current_emit_ctx().expect("with_return_tail called outside with_emit_ctx");
    let prev = ctx.in_return_tail.replace(value);
    let r = f();
    ctx.in_return_tail.set(prev);
    r
}

pub(super) fn is_static_method(name: &str) -> bool {
    current_emit_ctx()
        .map(|ctx| ctx.static_methods.borrow().contains(name))
        .unwrap_or(false)
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

/// Wrap `raw` with the str-coercion shape stamped by the decide
/// pass (`emit/rust2/decide/str_color.rs`). Single application
/// point so per-node match arms in `emit_expr_inner` can keep
/// producing the natural non-coerced shape; coercions land here
/// based on the `STR_TO_OWNED` / `STR_BORROW` bits once and don't
/// have to be re-derived per node kind.
///
/// Defensive parens around the inner emit keep the surrounding
/// expression context safe — `&` and `.to_string()` both have
/// surprising precedence when the inner is a method-call chain or
/// arithmetic expression.
fn apply_str_coercion(raw: String, e: &Expr) -> String {
    if e.decisions & super::decide::bits::STR_TO_OWNED != 0 {
        format!("({raw}).to_string()")
    } else if e.decisions & super::decide::bits::STR_BORROW != 0 {
        format!("&({raw})")
    } else {
        raw
    }
}

/// `true` when the str_color decide pass stamped a coercion bit
/// (`STR_TO_OWNED` or `STR_BORROW`) on `e`. Peephole gates in
/// `literal.rs`, `assign.rs`, `control.rs`, `send/coerce.rs` use
/// this to skip their own ad-hoc `.to_string()` insertions —
/// otherwise the literal site would double-coerce on top of the
/// decide pass's wrap.
pub(super) fn has_str_coercion(e: &Expr) -> bool {
    e.decisions
        & (super::decide::bits::STR_TO_OWNED | super::decide::bits::STR_BORROW)
        != 0
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
            if let Some(s) = narrowed_param_read(n, e.ty.as_ref()) {
                return s;
            }
            // Local-Var narrowing-from-Value: when the body-typer
            // narrows a Value-storage local (declared as
            // `serde_json::Value` / `Roundhouse::ParamValue`) to a
            // primitive via `is_a?(String/Integer/…)`, emit the
            // `serde_json::Value::as_<primitive>().unwrap()` conversion
            // at the Var read so the surrounding `if … { var } else
            // { default }` branches unify on the primitive's borrowed
            // form. Without this, the then-branch produces bare
            // `Value` and the else-branch produces the primitive — the
            // E0308 mismatch the `synth_from_raw`-emitted
            // `if raw_field.is_a?(String) { raw_field } else { "" }`
            // shape would otherwise trip. Restricted to primitive
            // narrowings (Str/Sym/Int/Bool/Float) — Hash/Array
            // narrowings keep their existing Cast-wrapped path.
            if let (Some(narrowed), Some(declared)) = (
                e.ty.as_ref(),
                local_var_ty(n).as_ref(),
            ) {
                use crate::ty::Ty;
                let declared_peeled = crate::emit::rust2::expr::util::peel_nil(declared);
                let declared_is_value = matches!(declared_peeled, Ty::Untyped | Ty::Record { .. })
                    || matches!(
                        declared_peeled,
                        Ty::Class { id, .. } if id.0.as_str() == "Roundhouse::ParamValue"
                    );
                if declared_is_value {
                    let coercion = match narrowed {
                        Ty::Str | Ty::Sym => Some("as_str().unwrap()"),
                        Ty::Int => Some("as_i64().unwrap()"),
                        Ty::Float => Some("as_f64().unwrap()"),
                        Ty::Bool => Some("as_bool().unwrap()"),
                        _ => None,
                    };
                    if let Some(c) = coercion {
                        return format!("{n}.{c}");
                    }
                }
            }
            // Stage 3 (#22): the `CLONE_AT` decide-pass bit is set
            // when this read site needs `.clone()` — the var is
            // read more than once in the method, this read is not
            // the lexically-last use of the name, and the value
            // type is non-Copy. Render appends `.clone()` unless we
            // are at a Send recv slot (auto-ref handles borrowing
            // there). Strict improvement over the prior name-set
            // approach, which over-cloned the lexically-last read.
            let at_send_recv = current_emit_ctx()
                .map(|ctx| ctx.suppress_var_clone.get())
                .unwrap_or(false);
            if e.decisions & super::decide::bits::CLONE_AT != 0 && !at_send_recv {
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
                if module_singleton_thread_local() {
                    // Request-scoped slot: borrow the thread-local
                    // RefCell instead of locking a process-wide mutex
                    // (which profiled as the dominant render-path
                    // serializer at c=64 — roundhouse#32).
                    return format!(
                        "{slot}.with(|__s| __s.borrow().clone()).unwrap_or_default()"
                    );
                }
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
        ExprNode::If { cond, then_branch, else_branch } => emit_if(cond, then_branch, else_branch),
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
                            // `.as_object()` yields a serde_json `Map`,
                            // which is a BTreeMap — iteration is already
                            // key-sorted (deterministic).
                            return format!(
                                "{recv_s}.as_object().unwrap().iter().for_each({closure})"
                            );
                        }
                        // String-keyed `HashMap` iterates in a random
                        // per-run order. Several runtime-Ruby surfaces
                        // render observable output from a `Hash#each` —
                        // notably `render_attrs`, whose HTML attribute
                        // order feeds Turbo's `data-turbo-track="reload"`
                        // element comparison; an unstable order makes
                        // Turbo force a full page reload after every form
                        // submit, defeating Drive (the e2e turbo/cable
                        // specs). Iterate sorted keys for a stable order.
                        // Sorted≠Rails insertion order, but compare's DOM
                        // diff is attribute-order-insensitive (BTreeMap),
                        // so byte-parity is unaffected. Bind `_m` so the
                        // collected borrow outlives the (possibly cloned)
                        // temporary receiver. Mirrors the go2 fix.
                        // Both `Ty::Str` and `Ty::Sym` keys render as a
                        // `String` HashMap key (see ty.rs: `Ty::Sym =>
                        // String`), so a Symbol-keyed hash iterates in the
                        // same random per-run order and needs the same
                        // sort for deterministic attribute output (e.g.
                        // render_attrs, whose param is `Hash[Symbol, …]`).
                        let key_is_str = matches!(
                            r.ty.as_ref(),
                            Some(crate::ty::Ty::Hash { key, .. })
                                if matches!(**key, crate::ty::Ty::Str | crate::ty::Ty::Sym)
                        );
                        if key_is_str {
                            return format!(
                                "{{ let _m = {recv_s}; \
                                 let mut _items: Vec<_> = _m.iter().collect(); \
                                 _items.sort_by(|a, b| a.0.cmp(b.0)); \
                                 _items.into_iter().for_each({closure}); }}"
                            );
                        }
                        return format!("{recv_s}.iter().for_each({closure})");
                    }
                    let is_array_after_peel = matches!(
                        r.ty.as_ref().map(peel_nil),
                        Some(crate::ty::Ty::Array { .. })
                    );
                    if is_array_after_peel && params.len() == 1 {
                        let p = params[0].as_str();
                        // `Option<Vec<T>>` recv (`Union<Nil, Array>`)
                        // takes the read-only `.iter().flatten()` chain
                        // below; the mutating no-clone path applies only
                        // to the plain-Vec `.iter_mut()` case.
                        let was_option = matches!(
                            r.ty.as_ref(),
                            Some(crate::ty::Ty::Union { variants })
                                if variants.iter().any(|v| matches!(v, crate::ty::Ty::Nil))
                        );
                        // A block that mutates its element (calls a
                        // `mutates_self` method on the param — the
                        // `_preload_<assoc>` distribute loop) must
                        // iterate the ORIGINAL via `iter_mut()` with no
                        // defensive `.clone()`, or the `&mut self`
                        // writes land on a throwaway temporary and are
                        // silently dropped (roundhouse#40). Suppress the
                        // multi-read clone the way every Send recv does
                        // (`emit_send_recv`). Read-only blocks keep the
                        // cloning path: it's harmless there and the
                        // clone supplies the mutable temporary that
                        // non-`mut` bindings (view params) need for
                        // `iter_mut()`. Guarded on the receiver being a
                        // `mut` bare Var (the distribute loop's freshly
                        // built `let mut results`) so we never emit
                        // `iter_mut()` against a binding the borrow
                        // checker would reject.
                        let recv_mutated = !was_option
                            && each_block_mutates_param(body, p)
                            && matches!(&*r.node, ExprNode::Var { name, .. } if is_mut_var(name.as_str()));
                        let recv_s = if recv_mutated { emit_send_recv(r) } else { emit_expr(r) };
                        let body_s = emit_expr(body);
                        let closure = if body_s.contains('\n') {
                            format!("|{p}| {{\n{};\n}}", indent(&body_s, 1))
                        } else {
                            format!("|{p}| {{ {body_s}; }}")
                        };
                        // `.iter().flatten().for_each(...)` for the
                        // Option recv so the closure receives `&T` from
                        // the inner Vec rather than `Vec<T>` from
                        // Option's iter (one item if Some). Read-only
                        // `iter()` because mutating-through-Option needs
                        // an as_mut + unwrap chain that's overkill for
                        // the read-only `parts << ...` framework Ruby.
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
            let base = emit_send(recv.as_ref(), method.as_str(), args, e.ty.as_ref());
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
        ExprNode::Seq { exprs } => emit_seq(exprs),
        ExprNode::Assign { target, value } => emit_assign(target, value),
        ExprNode::Return { value } => emit_return(value),
        ExprNode::While { cond, body, until_form } => emit_while(cond, body, *until_form),
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
        ExprNode::BoolOp { op, left, right, .. } => emit_bool_op(op, left, right),
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
        ExprNode::Case { scrutinee, arms } => emit_case(scrutinee, arms),
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
        // Ruby `raise "msg"` → Rust `panic!("{}", "msg")`. Cross-
        // target shape matches the TS IIFE/`throw` and Crystal `raise`
        // arms; `panic!` is a macro that diverges to `!`, so it
        // works in any expression position without an IIFE wrap.
        // Assertion failures (produced by the test_module lowerer's
        // `inline_assertions` pass) reach here as
        // `Raise { value: Lit::Str { ... } }`; emit them verbatim
        // so test bodies actually fail when assertions don't hold.
        ExprNode::Raise { value } => {
            format!("panic!(\"{{}}\", {})", emit_expr(value))
        }
        // Catch-all for IR shapes not yet implemented. Each new runtime
        // file in Phase 2 expands this until full coverage.
        other => format!("/* TODO rust2: ExprNode::{:?} */", std::mem::discriminant(other)),
    }
}

/// If `arg` is a Var (possibly wrapped in `.clone()`) with a recorded
/// Hash local_var_ty, return (K, V). Used by send.rs's Self::method
/// callee-back-propagation: when the callee's param is `Hash<_,
/// Untyped>` we need to know the arg's local-typed shape to decide
/// whether to insert the value-coercion transform.
pub(super) fn arg_hash_var_local_ty(arg: &Expr) -> Option<(crate::ty::Ty, crate::ty::Ty)> {
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

/// If `recv` is a Var whose `local_var_ty` was set via back-propagation
/// (`empty_hash_return_ty` in assign.rs), return its (K, V) types.
/// Gated on the back-propagation set so the Send `[]=` peephole only
/// coerces args when the recorded type is authoritative.
pub(super) fn recv_var_back_propagated_hash_kv(recv: &Expr) -> Option<(crate::ty::Ty, crate::ty::Ty)> {
    let name = match &*recv.node {
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        _ => return None,
    };
    if !is_back_propagated_hash(&name) {
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
/// emit tracker mirrors that.
pub(super) fn with_declared_vars_scope<R>(f: impl FnOnce() -> R) -> R {
    let ctx = current_emit_ctx().expect("with_declared_vars_scope called outside with_emit_ctx");
    let snapshot = ctx.declared_vars.borrow().clone();
    let r = f();
    *ctx.declared_vars.borrow_mut() = snapshot;
    r
}

