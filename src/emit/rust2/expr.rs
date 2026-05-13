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
    let mut_vars: std::collections::HashSet<String> = counts
        .into_iter()
        .filter_map(|(name, n)| if n > 1 { Some(name) } else { None })
        .collect();
    let prev_mut = MUT_VARS.with(|c| c.replace(mut_vars));
    let prev_declared =
        DECLARED_VARS.with(|c| c.replace(std::collections::HashSet::new()));
    let r = f();
    MUT_VARS.with(|c| *c.borrow_mut() = prev_mut);
    DECLARED_VARS.with(|c| *c.borrow_mut() = prev_declared);
    r
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
    with_return_tail(false, || emit_expr_inner(e))
}

/// Tail-preserving emit. Caller is responsible for ensuring this is
/// invoked only at tail positions of the enclosing function (e.g.,
/// `Seq`'s last expression, `Return`'s value, `If`'s branches when
/// the `If` itself is in tail position).
pub(super) fn emit_expr_tail(e: &Expr) -> String {
    emit_expr_inner(e)
}

fn emit_expr_inner(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        ExprNode::Ivar { name } => {
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
            // Branches inherit the enclosing function's return-tail
            // flag: an `if/else` in tail position has both branches in
            // tail position; the cond is not.
            if else_is_nil {
                return format!("if {} {{ {} }}", emit_expr(cond), emit_expr_tail(then_branch));
            }
            format!(
                "if {} {{ {} }} else {{ {} }}",
                emit_expr(cond),
                emit_expr_tail(then_branch),
                emit_expr_tail(else_branch),
            )
        }
        ExprNode::Send { recv, method, args, block, .. } => {
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
        ExprNode::Seq { exprs } => {
            // Rust statements are `;`-terminated; the last expression
            // is the block's value (no trailing `;`). Multi-statement
            // method bodies render natural Rust shape this way. The
            // tail expression inherits the enclosing function's
            // return-tail flag (`emit_expr_tail`) so e.g. a bare
            // `@field` at the end of a getter body becomes
            // `self.field.clone()`.
            let mut lines = Vec::with_capacity(exprs.len());
            let last = exprs.len().saturating_sub(1);
            for (i, e) in exprs.iter().enumerate() {
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
            }
            lines.join("\n")
        }
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
                "return".to_string()
            } else {
                format!("return {}", emit_expr_tail(value))
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
            // Ruby `a && b` / `a || b` map directly to Rust `&&` /
            // `||` — same short-circuit semantics, same precedence
            // relative to comparison ops. `and`/`or` surface forms
            // collapse to the symbol form on emit; Ruby's looser
            // precedence for the keyword form doesn't matter once
            // the IR has the tree shape locked in.
            let op_s = match op {
                crate::expr::BoolOpKind::And => "&&",
                crate::expr::BoolOpKind::Or => "||",
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        // Catch-all for IR shapes not yet implemented. Each new runtime
        // file in Phase 2 expands this until full coverage.
        other => format!("/* TODO rust2: ExprNode::{:?} */", std::mem::discriminant(other)),
    }
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
    // Non-empty hash literal: build via `HashMap::from([...])`. Works
    // for any K, V where K: Hash + Eq; relies on the surrounding
    // type context (let-binding or struct-field type) to infer the
    // HashMap's type parameters.
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("({}, {})", emit_expr(k), emit_expr(v)))
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
    let parts: Vec<String> = elements.iter().map(emit_expr).collect();
    format!("vec![{}]", parts.join(", "))
}

fn emit_assign(target: &LValue, value: &Expr) -> String {
    let rhs = emit_expr(value);
    match target {
        LValue::Var { name, .. } => {
            let name_str = name.as_str().to_string();
            let already_declared =
                DECLARED_VARS.with(|c| c.borrow().contains(&name_str));
            if already_declared {
                return format!("{name_str} = {rhs}");
            }
            let needs_mut = MUT_VARS.with(|c| c.borrow().contains(&name_str));
            DECLARED_VARS.with(|c| {
                c.borrow_mut().insert(name_str.clone());
            });
            if needs_mut {
                format!("let mut {name_str} = {rhs}")
            } else {
                format!("let {name_str} = {rhs}")
            }
        }
        LValue::Ivar { name } => {
            if in_constructor() {
                // Bind a local — the surrounding `pub fn new` body
                // closes with `Self { f1, f2, ... }` and the locals
                // become the field initializers. `mut` is uniformly
                // applied so subsequent index-assigns / re-assigns
                // inside initialize compile; the resulting `Self`
                // literal moves the local in regardless of mutability.
                format!("let mut {name} = {rhs}")
            } else {
                format!("self.{name} = {rhs}")
            }
        }
        LValue::Attr { recv, name } => format!("{}.{name} = {rhs}", emit_expr(recv)),
        LValue::Index { recv, index } => {
            format!("{}[{}] = {rhs}", emit_expr(recv), emit_expr(index))
        }
    }
}

fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    // Binary operators (==, !=, <, >, +, -, *, /) ingest as Send
    // with `method` as the operator name. Ruby `a == b` lowers to
    // `Send { recv: a, method: ==, args: [b] }`.
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
    // user-defined `<<` operators on other types stay intact.
    if method == "<<" && args.len() == 1 {
        if let Some(r) = recv {
            if matches!(r.ty.as_ref(), Some(crate::ty::Ty::Array { .. })) {
                return format!("{}.push({})", emit_expr(r), emit_expr(&args[0]));
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
            return format!("{}[{}]", emit_expr(r), emit_expr(&args[0]));
        }
        // Ruby `String#[](start, length)` — byte-slice with start +
        // length. Rust has no `Index<(usize, usize)>` for `&str`; lower
        // to a range slice. Args land here as `i64` from the body-typer,
        // hence the `as usize` casts. `&recv[..]` makes the result
        // `&str` regardless of whether `recv` is `String` or `&str`.
        // Caveat: `start_s` is duplicated in the emitted source; fine
        // for the literal/local arg shapes seen in practice (`str[0,
        // 10]`, `str[0, cutoff]`).
        if method == "[]" && args.len() == 2 {
            let recv_s = emit_expr(r);
            let start_s = emit_expr(&args[0]);
            let len_s = emit_expr(&args[1]);
            return format!(
                "&{recv_s}[({start_s}) as usize..(({start_s}) + ({len_s})) as usize]"
            );
        }
        if method == "[]=" && args.len() == 2 {
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
        if args_s.is_empty() {
            return format!("Self::{rewritten_method}()");
        }
        return format!("Self::{rewritten_method}({})", args_s.join(", "));
    }
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
    if args_s.is_empty() {
        format!("{recv_s}{dispatch}{rewritten_method}()")
    } else {
        format!("{recv_s}{dispatch}{rewritten_method}({})", args_s.join(", "))
    }
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
        "delete" => "remove",
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
pub(super) fn sanitize_ident(name: &str) -> String {
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
