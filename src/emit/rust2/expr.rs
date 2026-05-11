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

fn is_static_method(name: &str) -> bool {
    STATIC_METHODS.with(|c| c.borrow().contains(name))
}

pub(super) fn emit_expr(e: &Expr) -> String {
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => name.as_str().to_string(),
        ExprNode::Ivar { name } => {
            if in_constructor() {
                name.as_str().to_string()
            } else {
                format!("self.{name}")
            }
        }
        ExprNode::SelfRef => "self".to_string(),
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
            // shape for the `STMT if COND` modifier form (`return X
            // if cond`), with the else branch synthesized as `Nil`.
            // For those one-sided cases — else is the implicit Nil
            // AND the then branch diverges (Return/Raise) — emit the
            // statement form `if cond { stmt; }` rather than the
            // expression form `if cond { stmt } else { None }`. The
            // expression form is type-correct in Ruby (nil) but in
            // Rust would mismatch the surrounding return type
            // (`Option<Value>` vs `Value` for HWIA's `get` body, etc.)
            // and the `else { None }` is dead code anyway after the
            // Return.
            let then_diverges = matches!(
                &*then_branch.node,
                ExprNode::Return { .. } | ExprNode::Raise { .. }
            );
            let else_is_nil = matches!(
                &*else_branch.node,
                ExprNode::Lit { value: Literal::Nil }
            );
            if then_diverges && else_is_nil {
                return format!("if {} {{ {}; }}", emit_expr(cond), emit_expr(then_branch));
            }
            format!(
                "if {} {{ {} }} else {{ {} }}",
                emit_expr(cond),
                emit_expr(then_branch),
                emit_expr(else_branch),
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
            // method bodies render natural Rust shape this way.
            let mut lines = Vec::with_capacity(exprs.len());
            let last = exprs.len().saturating_sub(1);
            for (i, e) in exprs.iter().enumerate() {
                let s = emit_expr(e);
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
                format!("return {}", emit_expr(value))
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
    if matches!(&*r.node, ExprNode::SelfRef) && is_static_method(method) {
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

/// Strip trailing `?` / `!` from a Ruby identifier so Rust accepts
/// it as a function name. Public so `method.rs` can use the same
/// rule at `pub fn` definition sites.
pub(super) fn sanitize_ident(name: &str) -> String {
    let s = name.strip_suffix('?').unwrap_or(name);
    let s = s.strip_suffix('!').unwrap_or(s);
    s.to_string()
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
