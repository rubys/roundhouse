//! `rust2` method emit — `MethodDef` → Rust `fn` rendering.
//!
//! Phase 2.1 scope: emit a `pub fn` for class methods (the Module-
//! mode runtime files like `inflector.rb`). Instance methods + per-
//! type signature handling come in Phase 2.2+ as struct emit lands.

use std::fmt::Write;

use super::expr::emit_expr;
use super::ty::rust_ty;
use crate::dialect::{MethodDef, MethodReceiver};
use crate::ty::Ty;

/// Receiver-kind picker driven by the `mutates_self` heuristic
/// computed in `library.rs`. Returned tokens already include the
/// trailing comma when a method has additional params, so callers
/// can splice without re-checking emptiness.
fn render_self_receiver(mutates: bool) -> &'static str {
    if mutates { "&mut self" } else { "&self" }
}

/// Emit a single `MethodDef` as a `pub fn` Rust function.
///
/// Module-mode methods (every `def self.X` in `inflector.rb`,
/// `view_helpers.rb`, etc.) become free `pub fn`s at the file's
/// module scope. The file IS the namespace (Rust convention),
/// so no extra `mod NAME { ... }` wrapping is needed — callers use
/// `crate::inflector::pluralize(...)`.
pub(super) fn emit_module_method(m: &MethodDef) -> Result<String, String> {
    if !matches!(m.receiver, MethodReceiver::Class) {
        return Err(format!(
            "rust2::emit_module_method: only class methods supported in Module mode, \
             got instance method `{}`",
            m.name
        ));
    }
    let mut out = String::new();
    let params = render_params(m);
    let ret_clause = render_return(m);
    let fn_name = super::expr::sanitize_ident(m.name.as_str());
    writeln!(out, "pub fn {fn_name}{params}{ret_clause} {{").unwrap();
    let return_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    let param_types = collect_param_types(m);
    let body = super::expr::with_param_types(param_types, || super::expr::with_current_return_ty(return_ty.clone(), || super::expr::with_class_method_scope(|| {
        super::expr::with_method_scope(&m.body, || {
            // Same as emit_instance_method: enable the return-tail
            // flag so the body's top-level expression (Seq tail / If
            // branches in tail position) sees `in_return_tail() == true`
            // and can apply return-type-aware coercions (Some-wrap
            // for Option<T>-returning class methods, etc.).
            super::expr::with_return_tail(true, || super::expr::emit_expr_tail(&m.body))
        })
    })));
    // Function-tail Some(...) wrap — same logic as emit_instance_method.
    // Class methods that return Option<T> need their last expression
    // wrapped in `Some(...)` when the body-typer typed it as T.
    let body = if needs_function_tail_some_wrap(&m.body, return_ty.as_ref()) {
        wrap_last_expression_with_some(&body)
    } else {
        body
    };
    // Empty Ruby body (`def x; end`) renders as an empty body string.
    // Non-Unit return types still need a tail expression — inject the
    // type's default value. Hand-written abstract slots like
    // `_adapter_insert; end` in active_record/base.rb (load-bearing
    // empty per the spinel-AOT comment) take this path.
    let body = synthesize_default_body_if_empty(body, return_ty.as_ref());
    for line in body.lines() {
        writeln!(out, "    {line}").unwrap();
    }
    out.push_str("}\n");
    Ok(out)
}

fn render_params(m: &MethodDef) -> String {
    // Pull positional + kwarg sig params (filter Block — handled separately).
    // The Ruby `def` syntax omits `&block` from `m.params` but the
    // RBS-derived signature appends a `Block` Param at the end. Without
    // this filter, `def self.form_with(model:, ...)` (5 params) +
    // RBS-block (6th sig param) trips the length-mismatch fallback
    // and renders every param as `()`.
    let (sig_params_filtered, block_param): (
        Option<Vec<&crate::ty::Param>>,
        Option<&crate::ty::Param>,
    ) = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) => {
            let non_block: Vec<&crate::ty::Param> = params
                .iter()
                .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
                .collect();
            let block = params
                .iter()
                .find(|p| matches!(p.kind, crate::ty::ParamKind::Block));
            (Some(non_block), block)
        }
        _ => (None, None),
    };

    if m.params.is_empty() && block_param.is_none() && m.block_param.is_none() {
        return "()".to_string();
    }

    let sig_params = sig_params_filtered
        .as_ref()
        .filter(|sp| sp.len() == m.params.len());
    let mut parts: Vec<String> = m
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = p.name.as_str();
            match sig_params.and_then(|sp| sp.get(i)) {
                Some(sig_p) => format!("{name}: {}", rust_param_ty(&sig_p.ty)),
                None => format!("{name}: ()"),
            }
        })
        .collect();

    // Append `f: impl FnOnce(P1, P2, ...) -> R` when the signature
    // declares a block with a typed Fn signature. `yield(args)` in the
    // body emits as `f(args)` (per `emit_send`'s Yield arm); the closure
    // param is the receiving side of that call. Untyped/proc-typed
    // blocks fall back to a permissive `Box<dyn Fn>`-shaped closure —
    // body still references `f`, so the binding has to exist. RBS
    // block-signature parsing (src/rbs.rs::parse_function_type_to_fn)
    // produces `Ty::Fn`; older `Ty::Untyped` block placeholders use
    // the permissive fallback path.
    if let Some(bp) = block_param {
        // Prefer source-declared block-param name (from `MethodDef.block_param`)
        // when available so the body's `&block` Var refs resolve against the
        // same identifier. Fall back to "f" — the historical default — when
        // the method declares no source-level `&block` (RBS-only typed block,
        // e.g. `def foo() { ... } -> T` where the source uses bare `yield`).
        let name = m
            .block_param
            .as_ref()
            .map(|p| p.name.as_str())
            .unwrap_or("f");
        parts.push(render_block_closure_param(name, &bp.ty));
    } else if let Some(bp) = m.block_param.as_ref() {
        // RBS didn't supply a block signature, but the def-site
        // declared `&block` (carried in `MethodDef.block_param`).
        // Emit a heap-closure placeholder — `Box<dyn FnOnce>` is the
        // ABI-stable shape that forwarding (issue #25 stage 2)
        // needs to cross method boundaries; `impl FnOnce` here would
        // break the monomorphic gradient when a callee forwards the
        // block to another method without itself being generic.
        // Placeholder signature is zero-arg, unit-return; analyzer
        // back-prop (stage 3) will refine to match yield arity at
        // the body's `f(args)` sites. Param name uses the source-
        // declared identifier so forwarded `&block` Var refs in the
        // body resolve against the same name.
        parts.push(render_block_param_placeholder(bp.name.as_str()));
    }

    format!("({})", parts.join(", "))
}

/// Heap-closure placeholder rendered when the def-site declares
/// `&block` (via `MethodDef.block_param`) but no RBS signature
/// specifies the block's call shape. Permissive zero-arg, unit-return
/// — refinement to actual yield arity is stage-3 analyzer work.
/// The param name matches the source-declared identifier so that
/// `&block` forwarding in the body resolves against the same name.
fn render_block_param_placeholder(name: &str) -> String {
    format!("{name}: Box<dyn FnOnce()>")
}

/// Render the `<name>: impl FnOnce(...) -> R` parameter that backs
/// `yield` (or a forwarded `&block`) in the body. `block_ty` should
/// be a `Ty::Fn` parsed from the RBS block clause or refined by
/// `analyze::block_refine`; falls back to a permissive
/// `serde_json::Value`-typed closure when the block was left as
/// `Ty::Untyped`. The `name` argument lets callers thread the
/// source-declared identifier so body Var refs resolve against the
/// same parameter; legacy `yield`-only callers pass "f".
fn render_block_closure_param(name: &str, block_ty: &Ty) -> String {
    if let Ty::Fn { params, ret, .. } = block_ty {
        let arg_tys: Vec<String> = params
            .iter()
            .filter(|p| !matches!(p.kind, crate::ty::ParamKind::Block))
            .map(|p| rust_param_ty(&p.ty))
            .collect();
        let ret_s = rust_ty(ret);
        format!("{name}: impl FnOnce({}) -> {}", arg_tys.join(", "), ret_s)
    } else {
        // Permissive fallback for blocks the RBS didn't sign with a
        // signature. Author-signed Untyped accepts any args + returns
        // a String (the common form-helper shape).
        format!("{name}: impl FnOnce(serde_json::Value) -> String")
    }
}

fn render_return(m: &MethodDef) -> String {
    // Setter methods (`def x=`, `attr_writer :x`, `attr_accessor :x`)
    // have a Ruby-shape return type of the assigned value, but the
    // synthesized body is a bare `@x = value` assignment that returns
    // `()` in Rust. No framework call site uses the setter return
    // value (the convention is statement-position assignment), so
    // dropping the return type to `()` aligns body and signature
    // without losing reachable behavior. Detected on the original
    // Ruby method name (`m.name`) before sanitize_ident rewrites the
    // trailing `=` to a `set_` prefix.
    if m.name.as_str().ends_with('=') {
        return String::new();
    }
    match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => {
            if matches!(&**ret, Ty::Nil) {
                String::new()
            } else {
                format!(" -> {}", rust_ty(ret))
            }
        }
        _ => String::new(),
    }
}

/// Parameter type rendering — borrow-aware variant of `rust_ty`.
/// String params take `&str` (avoids forcing callers to clone), Vec
/// stays owned for now (Phase 2.1 scope; refine at Phase 4 when
/// closures + lifetimes pressure surfaces).
fn rust_param_ty(ty: &Ty) -> String {
    match ty {
        Ty::Str | Ty::Sym => "&str".to_string(),
        other => rust_ty(other),
    }
}

/// `true` when:
///   - return type is `Option<T>` (Union<T, Nil>)
///   - body's tail expression is T-typed (NOT Option<T>, NOT Nil)
///   - body's tail is NOT a Return statement (those are explicit)
///   - body's tail is NOT a divergent expression (raise/etc.)
fn needs_function_tail_some_wrap(body: &crate::expr::Expr, return_ty: Option<&Ty>) -> bool {
    use crate::expr::ExprNode;
    let return_is_option = matches!(
        return_ty,
        Some(Ty::Union { variants }) if variants.iter().any(|v| matches!(v, Ty::Nil))
    );
    if !return_is_option {
        return false;
    }
    let tail = tail_expression(body);
    // Skip if tail is a Return/Raise/diverging — already handles its
    // own return shape.
    match &*tail.node {
        ExprNode::Return { .. } => return false,
        _ => {}
    }
    // Temporal-reader intrinsic: `ActiveSupport.parse_db_time(...)` maps
    // (in `expr/send/mod.rs`) to `crate::rh_datetime::parse_db_time`,
    // which ALREADY returns `Option<DateTime<Utc>>`. Never wrap it in
    // `Some(...)` — the reader's `Option<...>` return type is satisfied
    // directly. The upstream type passes (`block_refine` / `decide`) can
    // re-stamp the tail's `.ty` off the synthesized `Union{Time, Nil}`,
    // so key off the call shape rather than the (unreliable here) type.
    if let ExprNode::Send { recv: Some(r), method, .. } = &*tail.node {
        if method.as_str() == "parse_db_time" {
            if let ExprNode::Const { path } = &*r.node {
                if path.last().map(|s| s.as_str()) == Some("ActiveSupport") {
                    return false;
                }
            }
        }
    }
    let tail_is_option = matches!(
        tail.ty.as_ref(),
        Some(Ty::Union { variants }) if variants.iter().any(|v| matches!(v, Ty::Nil))
    ) || matches!(tail.ty.as_ref(), Some(Ty::Nil) | Some(Ty::Bottom));
    tail.ty.is_some() && !tail_is_option
}

/// Walk into the body's tail expression: for a Seq, the last element;
/// otherwise the body itself. Recurses for nested Seqs so e.g. a body
/// of `Seq [stmt, Seq[a, b]]` returns `b`.
fn tail_expression(e: &crate::expr::Expr) -> &crate::expr::Expr {
    use crate::expr::ExprNode;
    match &*e.node {
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            tail_expression(exprs.last().unwrap())
        }
        _ => e,
    }
}

/// `true` when:
///   - return type is an owned class type T (`Ty::Class { ... }`)
///   - body's tail expression is `SelfRef` — i.e., the method ends
///     in bare `self` to return the receiver
/// The implicit receiver in instance methods is `&self` (or `&mut self`);
/// returning bare `self` produces `&Base` / `&mut Base`, which
/// doesn't match the owned `Base` return type. Wrapping the tail with
/// `.clone()` resolves it (struct emit derives Clone).
fn needs_function_tail_self_clone(body: &crate::expr::Expr, return_ty: Option<&Ty>) -> bool {
    use crate::expr::ExprNode;
    let return_is_owned_class = matches!(return_ty, Some(Ty::Class { .. }));
    if !return_is_owned_class {
        return false;
    }
    matches!(&*tail_expression(body).node, ExprNode::SelfRef)
}

/// Replace the last non-blank line `self` -> `self.clone()`. Leaves
/// other tail shapes untouched (those go through different paths).
fn clone_last_self_expression(body: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(|s| s.to_string()).collect();
    let last_idx = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
        .map(|(i, _)| i);
    if let Some(idx) = last_idx {
        let trimmed = lines[idx].trim_end_matches(';').to_string();
        let leading: String = lines[idx]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        if trimmed.trim_start() == "self" {
            lines[idx] = format!("{leading}self.clone()");
        }
    }
    lines.join("\n")
}

/// Wrap the last non-blank line of the body string in `Some(...)`.
/// The last line is the body's tail expression (single line or
/// multi-line — the emit produces one Rust expression per body tail).
/// Special-case bare `self` — `Some(self)` would produce
/// `Option<&Base>` (the receiver is `&self`/`&mut self`); use
/// `Some(self.clone())` to match the function's owned `Option<Base>`
/// return type. Struct emit derives Clone so this resolves cleanly.
fn wrap_last_expression_with_some(body: &str) -> String {
    let mut lines: Vec<String> = body.lines().map(|s| s.to_string()).collect();
    let last_idx = lines
        .iter()
        .enumerate()
        .rev()
        .find(|(_, l)| !l.trim().is_empty() && !l.trim_start().starts_with("//"))
        .map(|(i, _)| i);
    if let Some(idx) = last_idx {
        let trimmed = lines[idx].trim_end_matches(';').to_string();
        let leading: String = lines[idx]
            .chars()
            .take_while(|c| c.is_whitespace())
            .collect();
        let content = trimmed.trim_start();
        let wrapped = if content == "self" {
            "Some(self.clone())".to_string()
        } else {
            format!("Some({content})")
        };
        lines[idx] = format!("{leading}{wrapped}");
    }
    lines.join("\n")
}

/// Emit a single instance method. `mutates_self` decides the
/// receiver token (`&self` vs `&mut self`). `def initialize` is
/// special-cased to `pub fn new(...) -> Self` — Rust constructors
/// don't take a self receiver and return the constructed value.
///
/// The `_struct_name` and `_ivars` params are passed for the
/// `initialize → new` body synthesis (next commit's scope): the
/// constructor needs to render the user's body then close with a
/// `Self { f1, f2, .. }` literal initialized from the ivars. For
/// now they're unused — `initialize` emits the body as-is and the
/// resulting Rust will fail to compile until that synthesis lands.
pub(super) fn emit_instance_method(
    m: &MethodDef,
    mutates_self: bool,
    is_static: bool,
    _struct_name: &str,
    ivars: &[(String, Ty)],
) -> Result<String, String> {
    if !matches!(m.receiver, MethodReceiver::Instance) {
        return Err(format!(
            "rust2::emit_instance_method: expected instance method, got class method `{}`",
            m.name
        ));
    }
    let mut out = String::new();
    let is_init = m.name.as_str() == "initialize";
    let sanitized = super::expr::sanitize_ident(m.name.as_str());
    // `pub fn new(...)` constructors and static-safe methods both
    // drop the `&self` receiver — the latter were identified by
    // `library.rs::method_reads_self`. Call sites for static methods
    // route through `Self::method(args)` (see expr.rs::emit_send).
    let (fn_name, receiver): (&str, Option<&'static str>) = if is_init {
        ("new", None)
    } else if is_static {
        (sanitized.as_str(), None)
    } else {
        (sanitized.as_str(), Some(render_self_receiver(mutates_self)))
    };
    // Block-param injection: methods whose body uses `yield` get
    // an explicit closure parameter `mut f: impl FnMut(...)`. The
    // body's `yield x, y` sites then emit as `f(x, y)`.
    //
    // Arity + types come from the first observed Yield call in the
    // body. The RBS signature carries a `block: Option<Box<Ty>>`
    // slot, but the current RBS parser collapses block signatures
    // to `Ty::Untyped` — until that improves, body-derived
    // inference is the only signal. HWIA's `each` body yields
    // `(k: String, v: serde_json::Value)`, which is what we want.
    //
    // Fallback: when there's no yield but the def-site declared
    // `&block` (carried in `MethodDef.block_param`), emit a
    // `Box<dyn FnOnce>` placeholder. Heap-closure shape because
    // forwarding (stage 2: `ExprNode::ProcRef`) needs an ABI-stable
    // type that crosses method boundaries; analyzer back-prop
    // (stage 3) will refine the signature.
    let block_param = find_yield_signature(&m.body)
        .map(render_block_param_from_args)
        .or_else(|| {
            m.block_param
                .as_ref()
                .map(|bp| render_block_param_placeholder(bp.name.as_str()))
        });
    let params = render_instance_params(m, receiver, block_param.as_deref());
    let ret_clause = if is_init {
        " -> Self".to_string()
    } else {
        render_return(m)
    };
    writeln!(out, "pub fn {fn_name}{params}{ret_clause} {{").unwrap();
    // Thread the method's RBS-declared return type through to the
    // Return arm in `emit_expr` so `return nil` in a method typed
    // `-> T?` emits as `return None` instead of bare `return` (the
    // latter is E0069 in non-Unit-returning functions).
    let return_ty = match m.signature.as_ref() {
        Some(Ty::Fn { ret, .. }) => Some((**ret).clone()),
        _ => None,
    };
    // Build the param-name → declared-Ty map for the String coercion
    // logic in `emit_assign`. The body-typer doesn't always propagate
    // Option-ness from RBS to Var reads inside the body, so this is
    // the authoritative source for "is this param Option-typed".
    let param_types = collect_param_types(m);
    let body = super::expr::with_param_types(param_types, || super::expr::with_current_return_ty(return_ty.clone(), || super::expr::with_method_scope(&m.body, || {
        if is_init {
            let fields: Vec<String> = ivars.iter().map(|(n, _)| n.clone()).collect();
            super::expr::with_constructor_mode(fields, || emit_expr(&m.body))
        } else {
            // Body root is a function return position — let the
            // `Ivar` arm see `IN_RETURN_TAIL=true` so a getter shaped
            // `def field; @field; end` emits as `self.field.clone()`
            // for non-Copy field types. `emit_expr_tail` is the
            // flag-preserving variant; the plain `emit_expr` would
            // clear the flag at entry.
            super::expr::with_return_tail(true, || {
                super::expr::emit_expr_tail(&m.body)
            })
        }
    })));
    // Function-tail Some(...) wrap: if the method returns Option<T>
    // and the body's tail expression is T-typed (non-Option), wrap
    // the last line in `Some(...)`. The Ruby idiom returns the last
    // expression's value; the body-typer carries the per-expression
    // type but doesn't insert Option-wrapping itself — that's emit
    // work. Distinct from `Return { Lit::Nil }` (already handled in
    // expr.rs as `return None`); this is for the implicit tail.
    // Skip Some-wrap for setter methods — `render_return` drops the
    // signature's return type to `()` for `def x=` and `def []=`, but
    // `needs_function_tail_some_wrap` looks at the IR signature
    // (which the lowerer types as the assigned value's union). The
    // mismatch produces `Some(match name { … })` against a unit-
    // returning fn, which mangles into `Some(})` when
    // `wrap_last_expression_with_some` operates on the multi-line
    // match's closing brace.
    let is_setter = m.name.as_str().ends_with('=');
    let body = if !is_init && !is_setter && needs_function_tail_some_wrap(&m.body, return_ty.as_ref()) {
        wrap_last_expression_with_some(&body)
    } else if !is_init && needs_function_tail_self_clone(&m.body, return_ty.as_ref()) {
        // `def reload; ...; self; end` returning Base — `self` is
        // `&self` / `&mut self`, but the return type is the owned
        // `Base`. Clone the tail self to satisfy the owned shape.
        clone_last_self_expression(&body)
    } else {
        body
    };
    let body = if !is_init {
        synthesize_default_body_if_empty(body, return_ty.as_ref())
    } else {
        body
    };
    let body_lines: Vec<&str> = body.lines().collect();
    let last_idx = body_lines.len().saturating_sub(1);
    for (i, line) in body_lines.iter().enumerate() {
        if line.is_empty() {
            writeln!(out).unwrap();
            continue;
        }
        // In constructor mode the user body's tail is followed by
        // a `Self { ... }` literal that must be a separate statement —
        // the body's `Seq` emit leaves the tail un-terminated (Rust's
        // block-value convention), which would concatenate the tail
        // with the Self literal. Force-terminate the last user-body
        // line for the constructor case so the appended Self literal
        // is a distinct expression on its own line.
        // Unit-return tail terminator: when the method returns Nil
        // (the lowered controller-action shape — actions have no
        // explicit return; their bodies' tail is `self.render(...)`
        // returning String), the body's tail expression value would
        // become the function's return value and trip
        // "expected `()`, found `String`". Append `;` to discard.
        // Skips the case where the tail is already a block-shaped
        // expression (closes with `}`) since those are statements
        // with no value, or a return statement.
        let returns_unit = !is_init
            && matches!(return_ty.as_ref(), Some(Ty::Nil) | None);
        let needs_unit_terminator = returns_unit
            && i == last_idx
            && !line.trim_end().ends_with(';')
            && !line.trim_end().ends_with('{')
            && !line.trim_end().ends_with('}');
        let needs_terminator = is_init
            && i == last_idx
            && !line.trim_end().ends_with(';')
            && !line.trim_end().ends_with('{')
            && !line.trim_end().ends_with('}');
        if needs_terminator || needs_unit_terminator {
            writeln!(out, "    {line};").unwrap();
        } else {
            writeln!(out, "    {line}").unwrap();
        }
    }
    if is_init {
        // Close the constructor with `Self { f1, f2, ... }` — Rust's
        // struct-literal shorthand binds field names to local
        // variables of the same name, which is precisely what the
        // ivar→local rewrite above produces. Empty-ivar classes get
        // `Self {}` (still valid).
        //
        // Ivars not assigned in the `def initialize` body — fields
        // the struct declares but the constructor body didn't touch
        // (e.g. `@request_method`, `@request_path` on
        // ActionController::Base, where they get populated later by
        // the request dispatcher) — would otherwise reference
        // undeclared locals at the Self literal site. Emit a default-
        // initialized let binding for each so the literal compiles.
        let assigned: std::collections::HashSet<String> =
            collect_ivars_assigned_in_body(&m.body);
        for (fname, fty) in ivars {
            if !assigned.contains(fname) {
                writeln!(
                    out,
                    "    let {fname}: {} = {};",
                    super::ty::rust_ty(fty),
                    default_value_for_ty(fty),
                )
                .unwrap();
            }
        }
        let fields: Vec<&str> = ivars.iter().map(|(n, _)| n.as_str()).collect();
        writeln!(out, "    Self {{ {} }}", fields.join(", ")).unwrap();
    }
    out.push_str("}\n");
    Ok(out)
}

/// Walk the constructor body collecting names of ivars assigned
/// anywhere in it (via `@x = ...` or `self.x = ...` writes). Used
/// to find ivars the body didn't touch so the closing `Self { ... }`
/// literal can default-init them rather than referencing undeclared
/// locals.
fn collect_ivars_assigned_in_body(body: &crate::expr::Expr) -> std::collections::HashSet<String> {
    use crate::expr::{ExprNode, LValue};
    fn walk(e: &crate::expr::Expr, out: &mut std::collections::HashSet<String>) {
        match &*e.node {
            ExprNode::Assign { target: LValue::Ivar { name }, value } => {
                out.insert(name.as_str().to_string());
                walk(value, out);
            }
            ExprNode::Assign {
                target: LValue::Attr { recv, name },
                value,
            } if matches!(&*recv.node, ExprNode::SelfRef) => {
                out.insert(name.as_str().to_string());
                walk(value, out);
            }
            ExprNode::Assign { target, value } => {
                if let LValue::Attr { recv, .. } | LValue::Index { recv, .. } = target {
                    walk(recv, out);
                }
                walk(value, out);
            }
            // Lowerer-synthesized constructor setter: `Send { recv:
            // SelfRef, method: "<field>=" }` (Ruby's `self.col = value`
            // shape). Treat the same as a direct Ivar/Attr assign so
            // the default-init pass in `emit_instance_method` skips
            // these fields — the `Send` emit in `expr.rs` rewrites
            // them to `let <field> = …` in constructor context.
            ExprNode::Send { recv: Some(r), method, args, .. }
                if matches!(&*r.node, ExprNode::SelfRef)
                    && args.len() == 1
                    && method.as_str().ends_with('=')
                    && !method.as_str().starts_with('[') =>
            {
                let raw = method.as_str();
                out.insert(raw[..raw.len() - 1].to_string());
                for a in args {
                    walk(a, out);
                }
            }
            ExprNode::Seq { exprs } => exprs.iter().for_each(|e| walk(e, out)),
            ExprNode::If { cond, then_branch, else_branch } => {
                walk(cond, out);
                walk(then_branch, out);
                walk(else_branch, out);
            }
            ExprNode::While { cond, body, .. } => {
                walk(cond, out);
                walk(body, out);
            }
            ExprNode::Send { recv, args, block, .. } => {
                if let Some(r) = recv { walk(r, out); }
                args.iter().for_each(|a| walk(a, out));
                if let Some(b) = block { walk(b, out); }
            }
            ExprNode::Return { value } => walk(value, out),
            _ => {}
        }
    }
    let mut out = std::collections::HashSet::new();
    walk(body, &mut out);
    out
}

/// Default-value Rust expression for a field Ty — used to fill in
/// constructor-unassigned ivars at the Self literal. Mirrors the
/// `Default` impl shape for each `Ty` variant; types without a
/// natural Default fall back to `Default::default()` which the
/// compiler will reject (E0277) if the concrete type doesn't
/// derive Default — that's a clearer error than the
/// "cannot find value" the alternative produces.
/// If `body` is empty/whitespace-only and the function's declared
/// return type is non-Unit, replace it with a single line containing
/// the type's default value. Ruby `def x; end` returns nil, which the
/// emitter renders as an empty body string — Rust requires a tail
/// expression matching the return type. Hand-written abstract slots
/// in framework Ruby (e.g. `_adapter_insert; end` in
/// runtime/ruby/active_record/base.rb, intentionally empty per the
/// spinel-AOT polymorphic-dispatch comment) take this path; subclasses
/// override and the Base body should never actually be called.
fn synthesize_default_body_if_empty(body: String, return_ty: Option<&Ty>) -> String {
    if !body.trim().is_empty() {
        return body;
    }
    let ret = match return_ty {
        Some(t) => t,
        None => return body,
    };
    if matches!(ret, Ty::Nil) {
        return body;
    }
    default_value_for_ty(ret)
}

fn default_value_for_ty(ty: &Ty) -> String {
    match ty {
        Ty::Int => "0_i64".to_string(),
        Ty::Float => "0.0_f64".to_string(),
        Ty::Bool => "false".to_string(),
        Ty::Str | Ty::Sym => "String::new()".to_string(),
        Ty::Nil => "()".to_string(),
        Ty::Array { .. } => "Vec::new()".to_string(),
        Ty::Hash { .. } => "std::collections::HashMap::new()".to_string(),
        Ty::Untyped => "serde_json::Value::Null".to_string(),
        Ty::Union { variants } if variants.iter().any(|v| matches!(v, Ty::Nil)) => {
            "None".to_string()
        }
        _ => "Default::default()".to_string(),
    }
}

/// Build a map from each declared parameter name to its RBS-declared
/// Ty for the supplied method. Empty when the method has no
/// signature or no params. Used to thread param-shape information
/// into `emit_assign`'s coercion logic — the body-typer doesn't
/// always set the Option-ness on Var reads, so the param table is
/// the authoritative source.
fn collect_param_types(m: &MethodDef) -> std::collections::HashMap<String, Ty> {
    let mut out = std::collections::HashMap::new();
    let Some(Ty::Fn { params, .. }) = m.signature.as_ref() else {
        return out;
    };
    if params.len() != m.params.len() {
        return out;
    }
    for (p, sig_p) in m.params.iter().zip(params.iter()) {
        out.insert(p.name.as_str().to_string(), sig_p.ty.clone());
    }
    out
}

fn render_instance_params(
    m: &MethodDef,
    receiver: Option<&'static str>,
    block_param: Option<&str>,
) -> String {
    let sig_params = match m.signature.as_ref() {
        Some(Ty::Fn { params, .. }) if params.len() == m.params.len() => Some(params),
        _ => None,
    };
    let mut parts: Vec<String> = Vec::new();
    if let Some(r) = receiver {
        parts.push(r.to_string());
    }
    for (i, p) in m.params.iter().enumerate() {
        let name = p.name.as_str();
        let rendered = match sig_params.and_then(|sp| sp.get(i)) {
            Some(sig_p) => format!("{name}: {}", rust_param_ty(&sig_p.ty)),
            None => format!("{name}: ()"),
        };
        parts.push(rendered);
    }
    if let Some(bp) = block_param {
        parts.push(bp.to_string());
    }
    format!("({})", parts.join(", "))
}

/// Render the closure-typed param for a yield-using method.
/// `block_ty` is the Ty::Fn from the RBS signature's block slot.
/// Emits `mut f: impl FnMut(P1, P2, ...)` — `mut` so the closure
/// can be called repeatedly inside the body (Yield in a loop is the
/// common case), `impl FnMut` so the call site can pass any closure
/// matching the param types without forcing a generic on the type.
/// Walk a method body for the first `Yield { args }` site and
/// return its arg types. `None` means no Yield in the body — caller
/// skips block-param injection entirely. Body-derived signature
/// is consistent for a given method: Ruby's `yield a, b` shape
/// doesn't vary across call sites of the same method, so first-hit
/// arity is authoritative.
fn find_yield_signature(body: &crate::expr::Expr) -> Option<Vec<Ty>> {
    use crate::expr::ExprNode;
    match &*body.node {
        ExprNode::Yield { args } => {
            Some(args.iter().map(|a| a.ty.clone().unwrap_or(Ty::Untyped)).collect())
        }
        ExprNode::Seq { exprs } => exprs.iter().find_map(find_yield_signature),
        ExprNode::If { cond, then_branch, else_branch } => find_yield_signature(cond)
            .or_else(|| find_yield_signature(then_branch))
            .or_else(|| find_yield_signature(else_branch)),
        ExprNode::While { cond, body, .. } => {
            find_yield_signature(cond).or_else(|| find_yield_signature(body))
        }
        ExprNode::Send { recv, args, block, .. } => recv
            .as_ref()
            .and_then(find_yield_signature)
            .or_else(|| args.iter().find_map(find_yield_signature))
            .or_else(|| block.as_ref().and_then(find_yield_signature)),
        ExprNode::Assign { value, .. } => find_yield_signature(value),
        ExprNode::Return { value } => find_yield_signature(value),
        _ => None,
    }
}

/// Render `mut f: impl FnMut(P1, P2, ...)` from the inferred arg
/// types. Empty arg list emits `mut f: impl FnMut()`. `mut` is
/// uniformly applied so calling the closure inside a loop (the
/// typical case) compiles without per-caller `mut` annotations.
fn render_block_param_from_args(arg_tys: Vec<Ty>) -> String {
    let ps: Vec<String> = arg_tys.iter().map(rust_param_ty).collect();
    format!("mut f: impl FnMut({})", ps.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialect::{AccessorKind, MethodDef, MethodReceiver, Param};
    use crate::effect::EffectSet;
    use crate::emit::rust2::EmitCtx;
    use crate::emit::rust2::expr::with_emit_ctx;
    use crate::expr::{Expr, ExprNode};
    use crate::ident::Symbol;
    use crate::span::Span;

    fn empty_body() -> Expr {
        Expr::new(Span::synthetic(), ExprNode::Seq { exprs: vec![] })
    }

    fn base_module_method(name: &str) -> MethodDef {
        MethodDef {
            name: Symbol::from(name),
            receiver: MethodReceiver::Class,
            params: vec![],
            body: empty_body(),
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        }
    }

    #[test]
    fn module_method_with_block_param_emits_box_fnonce_placeholder() {
        let mut m = base_module_method("foo");
        m.block_param = Some(Param::positional(Symbol::from("blk")));
        let emitted = with_emit_ctx(EmitCtx::default(), || emit_module_method(&m)).expect("emit");
        assert!(
            emitted.contains("blk: Box<dyn FnOnce()>"),
            "expected Box<dyn FnOnce()> placeholder for &block-declaring method, got:\n{emitted}"
        );
    }

    #[test]
    fn module_method_without_block_param_unchanged() {
        let m = base_module_method("bar");
        let emitted = with_emit_ctx(EmitCtx::default(), || emit_module_method(&m)).expect("emit");
        assert!(
            !emitted.contains("FnOnce") && !emitted.contains("FnMut"),
            "method without block_param should not gain a closure param, got:\n{emitted}"
        );
        assert!(
            emitted.contains("pub fn bar()"),
            "expected zero-param signature, got:\n{emitted}"
        );
    }

    #[test]
    fn block_forwarding_var_in_send_block_slot_emits_bare_identifier() {
        // `def foo(&blk); bar(&blk); end` shape — the def-site `&blk`
        // populates `MethodDef.block_param`; the call-site `&blk`
        // ingests as `Send { method: bar, block: Some(Var("blk")) }`
        // (issue #25 stage 2 ingest, src/ingest/expr.rs).
        // Emit must (a) declare the closure param with the source name
        // and (b) splice the same name as the forwarded closure arg
        // so the body references the parameter that's actually in
        // scope.
        let blk_name = Symbol::from("blk");
        let bar_call = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("bar"),
                args: vec![],
                block: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var {
                        id: crate::ident::VarId(0),
                        name: blk_name.clone(),
                    },
                )),
                parenthesized: true,
            },
        );
        let m = MethodDef {
            name: Symbol::from("foo"),
            receiver: MethodReceiver::Class,
            params: vec![],
            body: bar_call,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: Some(Param::positional(blk_name)),
        };
        let emitted = with_emit_ctx(EmitCtx::default(), || emit_module_method(&m)).expect("emit");
        assert!(
            emitted.contains("blk: Box<dyn FnOnce()>"),
            "expected Box<dyn FnOnce()> with source name, got:\n{emitted}"
        );
        assert!(
            emitted.contains("bar(blk)"),
            "expected forwarded `bar(blk)` referencing the same name, got:\n{emitted}"
        );
    }

    #[test]
    fn block_refine_propagates_callee_sig_to_forwarder_emit() {
        // End-to-end: a class with a typed-block callee + a forwarder.
        // After `block_refine::propagate`, the forwarder's signature
        // gains the callee's block sig — and rust2 emit's existing
        // `render_block_closure_param` path produces the matching
        // `impl FnOnce` shape instead of the `Box<dyn FnOnce()>`
        // placeholder. Issue #25 stage 3.
        use crate::analyze::block_refine;
        use crate::dialect::LibraryClass;
        use crate::ident::ClassId;
        use crate::ty::{Param as TyParam, ParamKind, Ty};

        let blk = Symbol::from("blk");
        let block_sig = Ty::Fn {
            params: vec![TyParam {
                name: Symbol::new("k"),
                ty: Ty::Str,
                kind: ParamKind::Required,
            }],
            block: None,
            ret: Box::new(Ty::Nil),
            effects: EffectSet::pure(),
        };
        let callee = MethodDef {
            name: Symbol::from("each"),
            receiver: MethodReceiver::Class,
            params: vec![],
            body: empty_body(),
            signature: Some(Ty::Fn {
                params: vec![TyParam {
                    name: Symbol::new("block"),
                    ty: block_sig.clone(),
                    kind: ParamKind::Block,
                }],
                block: Some(Box::new(block_sig)),
                ret: Box::new(Ty::Nil),
                effects: EffectSet::pure(),
            }),
            effects: EffectSet::pure(),
            enclosing_class: Some(Symbol::from("Holder")),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: None,
        };
        let send_each = Expr::new(
            Span::synthetic(),
            ExprNode::Send {
                recv: None,
                method: Symbol::from("each"),
                args: vec![],
                block: Some(Expr::new(
                    Span::synthetic(),
                    ExprNode::Var { id: crate::ident::VarId(0), name: blk.clone() },
                )),
                parenthesized: true,
            },
        );
        let forwarder = MethodDef {
            name: Symbol::from("forwarder"),
            receiver: MethodReceiver::Class,
            params: vec![],
            body: send_each,
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: Some(Symbol::from("Holder")),
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: Some(Param::positional(blk)),
        };
        let mut class = LibraryClass {
            name: ClassId(Symbol::from("Holder")),
            is_module: false,
            parent: None,
            includes: vec![],
            methods: vec![callee, forwarder],
            origin: None,
            constants: Vec::new(),
        };
        block_refine::propagate_one(&mut class);
        let fwd = class
            .methods
            .iter()
            .find(|m| m.name.as_str() == "forwarder")
            .unwrap();
        let emitted = with_emit_ctx(EmitCtx::default(), || emit_module_method(fwd)).expect("emit");
        // Refined signature → existing `render_block_closure_param` path fires.
        // Placeholder `Box<dyn FnOnce()>` must NOT appear; the typed
        // `impl FnOnce(String) -> ()` shape must.
        assert!(
            !emitted.contains("Box<dyn FnOnce()>"),
            "placeholder should be displaced by refined sig, got:\n{emitted}"
        );
        assert!(
            emitted.contains("blk: impl FnOnce(&str)"),
            "expected refined `blk: impl FnOnce(&str) -> _` shape (param name matches \
             source-declared `&blk` so body's `each(blk)` Var resolves), got:\n{emitted}"
        );
    }

    #[test]
    fn instance_method_with_block_param_no_yield_emits_placeholder() {
        let m = MethodDef {
            name: Symbol::from("baz"),
            receiver: MethodReceiver::Instance,
            params: vec![],
            body: empty_body(),
            signature: None,
            effects: EffectSet::pure(),
            enclosing_class: None,
            kind: AccessorKind::Method,
            is_async: false,
            mutates_self: false,
            block_param: Some(Param::positional(Symbol::from("blk"))),
        };
        let emitted = with_emit_ctx(EmitCtx::default(), || {
            emit_instance_method(&m, false, false, "Dummy", &[])
        })
        .expect("emit");
        assert!(
            emitted.contains("blk: Box<dyn FnOnce()>"),
            "expected Box<dyn FnOnce()> placeholder, got:\n{emitted}"
        );
        // Body has no yield, so the FnMut path must NOT trigger.
        assert!(
            !emitted.contains("FnMut"),
            "FnMut path should not trigger for yield-less body, got:\n{emitted}"
        );
    }
}
