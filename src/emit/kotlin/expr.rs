//! `Expr` → Kotlin source.
//!
//! Phase 2 coverage: the node kinds the lowered model bodies exercise.
//! Modeled on `src/emit/crystal/expr.rs` but rendered Kotlin-idiomatic —
//! camelCase identifiers (`super::naming::camel`), `?:` for nil-coalescing
//! `||`, `when` for `case`, trailing lambdas for blocks, and `var`/`val`
//! inference for local assignments.
//!
//! Untyped/edge nodes that don't map cleanly emit a `/* TODO kind */`
//! marker rather than panicking, so a full model still renders and the
//! gaps are visible in the output.
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use crate::expr::{Arm, BoolOpKind, Expr, ExprNode, InterpPart, LValue, Literal, Pattern};

use super::naming::camel;
use super::ty::kotlin_ty;

thread_local! {
    /// Local names already declared in the current method body (so the
    /// first `Assign` emits `val`/`var` and later ones emit bare `=`).
    static DECLARED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Local names assigned more than once → declared `var` (else `val`).
    static REASSIGNED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// For locals first assigned `nil`, the nullable Kotlin type taken
    /// from a later non-nil assignment — so `var x = null` (which Kotlin
    /// infers as `Nothing?`) becomes `var x: T? = null`.
    static NIL_TYPES: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// Reset per-method local-decl tracking and pre-scan the body for
/// reassignment counts. Called by `library::emit_method` before the body
/// is rendered.
pub(super) fn begin_method(body: &Expr) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut nil_types: HashMap<String, String> = HashMap::new();
    count_assigns(body, &mut counts, &mut nil_types);
    DECLARED.with(|d| d.borrow_mut().clear());
    REASSIGNED.with(|r| {
        let mut set = r.borrow_mut();
        set.clear();
        for (name, n) in counts {
            if n > 1 {
                set.insert(name);
            }
        }
    });
    NIL_TYPES.with(|t| *t.borrow_mut() = nil_types);
}

fn count_assigns(
    e: &Expr,
    counts: &mut HashMap<String, usize>,
    nil_types: &mut HashMap<String, String>,
) {
    if let ExprNode::Assign { target: LValue::Var { name, .. }, value } = &*e.node {
        let cn = camel(name.as_str());
        *counts.entry(cn.clone()).or_insert(0) += 1;
        // Record the first non-nil assigned type so a `nil`-first local
        // gets a real nullable declaration type.
        if !nil_types.contains_key(&cn) {
            if let Some(ty) = value.ty.as_ref() {
                if !matches!(ty, crate::ty::Ty::Nil) {
                    let mut kt = kotlin_ty(ty);
                    if !kt.ends_with('?') {
                        kt.push('?');
                    }
                    nil_types.insert(cn, kt);
                }
            }
        }
    }
    for child in children(e) {
        count_assigns(child, counts, nil_types);
    }
}

/// Shallow child-expression walk — enough for the assignment pre-scan.
fn children(e: &Expr) -> Vec<&Expr> {
    let mut v = Vec::new();
    match &*e.node {
        ExprNode::Seq { exprs } => v.extend(exprs.iter()),
        ExprNode::If { cond, then_branch, else_branch } => {
            v.push(cond);
            v.push(then_branch);
            v.push(else_branch);
        }
        ExprNode::While { cond, body, .. } => {
            v.push(cond);
            v.push(body);
        }
        ExprNode::Assign { value, .. } => v.push(value),
        ExprNode::Case { scrutinee, arms } => {
            v.push(scrutinee);
            for a in arms {
                v.push(&a.body);
            }
        }
        ExprNode::Send { recv, args, block, .. } => {
            if let Some(r) = recv {
                v.push(r);
            }
            v.extend(args.iter());
            if let Some(b) = block {
                v.push(b);
            }
        }
        ExprNode::BoolOp { left, right, .. } => {
            v.push(left);
            v.push(right);
        }
        ExprNode::Return { value } | ExprNode::Raise { value } => v.push(value),
        ExprNode::Lambda { body, .. } => v.push(body),
        _ => {}
    }
    v
}

pub fn emit_expr(e: &Expr) -> String {
    emit_node(&e.node, e)
}

pub fn emit_expr_for_runtime(e: &Expr) -> String {
    emit_expr(e)
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| if l.is_empty() { String::new() } else { format!("    {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_empty_branch(e: &Expr) -> bool {
    matches!(&*e.node, ExprNode::Seq { exprs } if exprs.is_empty())
        || matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
}

fn emit_node(n: &ExprNode, e: &Expr) -> String {
    match n {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Var { name, .. } => camel(name.as_str()),
        // Instance variable → property reference.
        ExprNode::Ivar { name } => camel(name.as_str()),
        ExprNode::SelfRef => "this".to_string(),
        ExprNode::Const { path } => path
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .join("."),
        ExprNode::Hash { entries, .. } => emit_hash(entries),
        ExprNode::Array { elements, .. } => emit_array(elements, e),
        ExprNode::StringInterp { parts } => emit_string_interp(parts),
        ExprNode::BoolOp { op, left, right, .. } => emit_bool_op(*op, left, right, e),
        ExprNode::Send { recv, method, args, block, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args, block.as_ref())
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            emit_if(cond, then_branch, else_branch)
        }
        ExprNode::Case { scrutinee, arms } => emit_case(scrutinee, arms),
        ExprNode::Seq { exprs } => exprs
            .iter()
            .map(emit_expr)
            .collect::<Vec<_>>()
            .join("\n"),
        ExprNode::Assign { target, value } => emit_assign(target, value),
        ExprNode::Return { value } => {
            if matches!(&*value.node, ExprNode::Lit { value: Literal::Nil }) {
                "return".to_string()
            } else {
                format!("return {}", emit_expr(value))
            }
        }
        ExprNode::While { cond, body, until_form } => {
            let c = emit_expr(cond);
            let c = if *until_form { format!("!({c})") } else { c };
            format!("while ({c}) {{\n{}\n}}", indent(&emit_expr(body)))
        }
        ExprNode::Raise { value } => emit_raise(value),
        // `super()` in `initialize` has no Kotlin method-body analog
        // (super-constructor calls live in the class header). Phase 2
        // emits a placeholder; Phase 3 wires the base properly.
        ExprNode::Super { .. } => "/* super() */".to_string(),
        ExprNode::Cast { value, target_ty } => emit_cast(value, target_ty),
        ExprNode::Lambda { params, body, .. } => emit_lambda(params, body),
        ExprNode::RescueModifier { expr, fallback } => format!(
            "try {{ {} }} catch (e: Exception) {{ {} }}",
            emit_expr(expr),
            emit_expr(fallback)
        ),
        other => format!("/* TODO {} */", other.kind_str()),
    }
}

fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "null".to_string(),
        Literal::Bool { value } => value.to_string(),
        // `Ty::Int → Long`, and Kotlin won't compare/assign across
        // numeric types, so integer literals carry the `L` suffix. (The
        // hand-written `Db` primitive correspondingly takes `Long`
        // indices.)
        Literal::Int { value } => format!("{value}L"),
        Literal::Float { value } => {
            if value.fract() == 0.0 {
                format!("{value:.1}")
            } else {
                format!("{value}")
            }
        }
        Literal::Str { value } => format!("\"{}\"", escape_str(value)),
        // No symbol type in Kotlin → string.
        Literal::Sym { value } => format!("\"{}\"", escape_str(value.as_str())),
        Literal::Regex { pattern, .. } => format!("Regex(\"{}\")", escape_str(pattern)),
    }
}

fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            // Kotlin has no `\f` escape; use the unicode form.
            '\u{0C}' => out.push_str("\\u000C"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04X}", c as u32)),
            _ => out.push(c),
        }
    }
    out
}

fn emit_hash(entries: &[(Expr, Expr)]) -> String {
    if entries.is_empty() {
        return "mutableMapOf<String, Any?>()".to_string();
    }
    let pairs: Vec<String> = entries
        .iter()
        .map(|(k, v)| format!("{} to {}", emit_expr(k), emit_expr(v)))
        .collect();
    format!("mutableMapOf<String, Any?>({})", pairs.join(", "))
}

fn emit_array(elements: &[Expr], e: &Expr) -> String {
    if elements.is_empty() {
        // Use the annotated element type when present, else Any?.
        if let Some(crate::ty::Ty::Array { elem }) = e.ty.as_ref() {
            return format!("mutableListOf<{}>()", kotlin_ty(elem));
        }
        return "mutableListOf<Any?>()".to_string();
    }
    let els: Vec<String> = elements.iter().map(emit_expr).collect();
    format!("mutableListOf({})", els.join(", "))
}

fn emit_string_interp(parts: &[InterpPart]) -> String {
    let mut out = String::from("\"");
    for part in parts {
        match part {
            InterpPart::Text { value } => out.push_str(&escape_str(value)),
            InterpPart::Expr { expr } => {
                out.push_str(&format!("${{{}}}", emit_expr(expr)));
            }
        }
    }
    out.push('"');
    out
}

fn emit_bool_op(op: BoolOpKind, left: &Expr, right: &Expr, e: &Expr) -> String {
    let l = emit_expr(left);
    let r = emit_expr(right);
    match op {
        BoolOpKind::And => format!("{l} && {r}"),
        // `||` is logical-or for Bool results, but Ruby's `x || default`
        // nil-coalescing idiom maps to Kotlin's `?:` when the result
        // isn't a Bool.
        BoolOpKind::Or => {
            if matches!(e.ty.as_ref(), Some(crate::ty::Ty::Bool)) {
                format!("{l} || {r}")
            } else {
                format!("{l} ?: {r}")
            }
        }
    }
}

fn emit_if(cond: &Expr, then_branch: &Expr, else_branch: &Expr) -> String {
    let c = emit_expr(cond);
    let then = indent(&emit_expr(then_branch));
    if is_empty_branch(else_branch) {
        format!("if ({c}) {{\n{then}\n}}")
    } else {
        let els = indent(&emit_expr(else_branch));
        format!("if ({c}) {{\n{then}\n}} else {{\n{els}\n}}")
    }
}

fn emit_case(scrutinee: &Expr, arms: &[Arm]) -> String {
    let s = emit_expr(scrutinee);
    let mut lines = Vec::new();
    let mut has_else = false;
    for arm in arms {
        let body = emit_expr(&arm.body);
        let body_block = if body.contains('\n') {
            format!("{{\n{}\n}}", indent(&body))
        } else {
            body
        };
        match &arm.pattern {
            Pattern::Wildcard | Pattern::Bind { .. } => {
                has_else = true;
                lines.push(format!("    else -> {body_block}"));
            }
            Pattern::Lit { value } => {
                lines.push(format!("    {} -> {body_block}", emit_literal(value)));
            }
            other => {
                lines.push(format!("    /* TODO pattern {other:?} */ else -> {body_block}"));
                has_else = true;
            }
        }
    }
    if !has_else {
        lines.push("    else -> null".to_string());
    }
    format!("when ({s}) {{\n{}\n}}", lines.join("\n"))
}

fn emit_assign(target: &LValue, value: &Expr) -> String {
    let val = emit_expr(value);
    match target {
        LValue::Var { name, .. } => {
            let n = camel(name.as_str());
            let already = DECLARED.with(|d| d.borrow().contains(&n));
            if already {
                format!("{n} = {val}")
            } else {
                let is_var = REASSIGNED.with(|r| r.borrow().contains(&n));
                DECLARED.with(|d| {
                    d.borrow_mut().insert(n.clone());
                });
                let kw = if is_var { "var" } else { "val" };
                // `var x = null` infers `Nothing?`; annotate from a later
                // non-nil assignment when we have one.
                let is_nil = matches!(&*value.node, ExprNode::Lit { value: Literal::Nil });
                if is_nil {
                    if let Some(ty) = NIL_TYPES.with(|t| t.borrow().get(&n).cloned()) {
                        return format!("{kw} {n}: {ty} = {val}");
                    }
                    return format!("{kw} {n}: Any? = {val}");
                }
                format!("{kw} {n} = {val}")
            }
        }
        LValue::Ivar { name } => format!("{} = {val}", camel(name.as_str())),
        LValue::Attr { recv, name } => {
            format!("{}.{} = {val}", emit_expr(recv), camel(name.as_str()))
        }
        LValue::Index { recv, index } => {
            format!("{}[{}] = {val}", emit_expr(recv), emit_expr(index))
        }
        LValue::Const { path } => {
            let p = path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".");
            format!("{p} = {val}")
        }
    }
}

fn emit_raise(value: &Expr) -> String {
    match &*value.node {
        ExprNode::Lit { value: Literal::Str { .. } } | ExprNode::StringInterp { .. } => {
            format!("throw RuntimeException({})", emit_expr(value))
        }
        _ => format!("throw {}", emit_expr(value)),
    }
}

/// Kotlin's `as` is a checked reference cast — it does NOT convert
/// between numeric types or stringify. The lowerer inserts `Cast` at
/// untyped-row boundaries to mean "coerce to this column type", so map
/// numeric/string targets to the conversion functions; reference targets
/// keep `as`.
fn emit_cast(value: &Expr, target_ty: &crate::ty::Ty) -> String {
    use crate::ty::Ty;
    let v = emit_expr(value);
    match target_ty {
        Ty::Int => format!("({v}).toString().toLong()"),
        Ty::Float => format!("({v}).toString().toDouble()"),
        Ty::Str | Ty::Sym => format!("({v}).toString()"),
        _ => format!("({v} as {})", kotlin_ty(target_ty)),
    }
}

/// `recv[begin..]` / `recv[begin..end]` → Kotlin `substring`. Indices are
/// `Long` (Ty::Int → Long), so `.toInt()` for the String API.
fn emit_slice_range(
    rs: &str,
    begin: Option<&Expr>,
    end: Option<&Expr>,
    exclusive: bool,
) -> String {
    let b = begin.map(emit_expr).unwrap_or_else(|| "0L".to_string());
    match end {
        None => format!("{rs}.substring(({b}).toInt())"),
        Some(e) => {
            let e = emit_expr(e);
            let end_idx = if exclusive {
                format!("({e}).toInt()")
            } else {
                format!("(({e}) + 1).toInt()")
            };
            format!("{rs}.substring(({b}).toInt(), {end_idx})")
        }
    }
}

fn emit_lambda(params: &[crate::ident::Symbol], body: &Expr) -> String {
    let body_s = emit_expr(body);
    if params.is_empty() {
        format!("{{ {body_s} }}")
    } else {
        let ps: Vec<String> = params.iter().map(|p| camel(p.as_str())).collect();
        format!("{{ {} -> {body_s} }}", ps.join(", "))
    }
}

/// Methods that look like 0-arg attribute reads but are real method calls
/// (need `()` in Kotlin). Everything else with a receiver and no args is
/// emitted as property access.
fn forces_parens(method: &str) -> bool {
    matches!(
        method,
        "save" | "save!" | "destroy" | "destroy!" | "reload" | "validate" | "dup" | "clone"
    )
}

fn emit_send(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();

    // Constructor: `X.new(...)` → `X(...)`.
    if method == "new" {
        if let Some(r) = recv {
            return format!("{}({})", emit_expr(r), args_s.join(", "));
        }
    }

    // Attribute setter: `recv.foo = v` arrives as a Send named `foo=`.
    if let (Some(r), 1) = (recv, args.len()) {
        if method.ends_with('=') && !matches!(method, "==" | "!=" | "<=" | ">=") {
            let base = &method[..method.len() - 1];
            return format!("{}.{} = {}", emit_expr(r), camel(base), args_s[0]);
        }
    }

    // `is_a?(Class)` → Kotlin `is` / boolean compare.
    if method == "is_a?" && args.len() == 1 {
        if let (Some(r), ExprNode::Const { path }) = (recv, &*args[0].node) {
            let rs = emit_expr(r);
            let last = path.last().map(|s| s.as_str()).unwrap_or("");
            return match last {
                "TrueClass" => format!("({rs} == true)"),
                "FalseClass" => format!("({rs} == false)"),
                "Integer" => format!("{rs} is Long"),
                "Float" => format!("{rs} is Double"),
                "String" => format!("{rs} is String"),
                "Numeric" => format!("{rs} is Number"),
                other => format!("{rs} is {}", other.rsplit("::").next().unwrap_or(other)),
            };
        }
    }

    // `recv.gsub(pattern, hash)` → regex replace with a map lookup.
    if method == "gsub" && args.len() == 2 {
        if let Some(r) = recv {
            return format!(
                "{}.replace({}) {{ (({})[it.value] ?: it.value).toString() }}",
                args_s[0],
                emit_expr(r),
                args_s[1]
            );
        }
    }

    // Indexing / slicing.
    if method == "[]" {
        if let Some(r) = recv {
            let rs = emit_expr(r);
            if args.len() == 1 {
                // `str[a..]` / `str[a..b]` slice.
                if let ExprNode::Range { begin, end, exclusive } = &*args[0].node {
                    return emit_slice_range(&rs, begin.as_ref(), end.as_ref(), *exclusive);
                }
                return format!("{rs}[{}]", args_s[0]);
            }
            if args.len() == 2 {
                // Ruby `str[start, len]` → `substring(start, start + len)`.
                let start = &args_s[0];
                let len = &args_s[1];
                return format!(
                    "{rs}.substring(({start}).toInt(), (({start}) + ({len})).toInt())"
                );
            }
        }
    }

    // Binary operators with a receiver and one arg.
    if let (Some(r), 1) = (recv, args.len()) {
        if matches!(
            method,
            "+" | "-" | "*" | "/" | "%" | "<" | ">" | "<=" | ">=" | "==" | "!=" | "&&" | "||"
        ) {
            return format!("{} {} {}", emit_expr(r), method, args_s[0]);
        }
        // `<<` push → MutableList.add.
        if method == "<<" {
            return format!("{}.add({})", emit_expr(r), args_s[0]);
        }
        // Hash key test.
        if method == "key?" || method == "has_key?" {
            return format!("{}.containsKey({})", emit_expr(r), args_s[0]);
        }
    }
    if let (Some(r), 2) = (recv, args.len()) {
        if method == "[]=" {
            return format!("{}[{}] = {}", emit_expr(r), args_s[0], args_s[1]);
        }
    }

    // Zero-arg receiver sends: builtin coercions, then property vs method.
    if let (Some(r), true) = (recv, args.is_empty() && block.is_none()) {
        let rs = emit_expr(r);
        match method {
            "nil?" => return format!("({rs} == null)"),
            "to_s" => return format!("{rs}.toString()"),
            "to_i" => return format!("{rs}.toString().toLong()"),
            "to_f" => return format!("{rs}.toString().toDouble()"),
            "empty?" => return format!("{rs}.isEmpty()"),
            "any?" => return format!("{rs}.isNotEmpty()"),
            "length" => return format!("{rs}.length"),
            "size" => return format!("{rs}.size"),
            // No-ops in Kotlin — drop, keep the receiver.
            "freeze" | "dup" | "to_a" => return rs,
            _ => {}
        }
        // A `Const` receiver (a class / object like `Db`, `Broadcasts`)
        // means a 0-arg *method* call, not a property read.
        if matches!(&*r.node, ExprNode::Const { .. }) {
            return format!("{rs}.{}()", camel(method));
        }
        if !forces_parens(method) && !method.ends_with('?') && !method.ends_with('!') {
            // Attribute read on an instance.
            return format!("{rs}.{}", camel(method));
        }
    }

    // Block → Kotlin trailing lambda (`.each` → `.forEach`).
    if let Some(b) = block {
        let kt_method = if method == "each" { "forEach".to_string() } else { camel(method) };
        let lam = emit_expr(b);
        let base = match recv {
            Some(r) => format!("{}.{kt_method}", emit_expr(r)),
            None => kt_method,
        };
        if args_s.is_empty() {
            return format!("{base} {lam}");
        }
        return format!("{base}({}) {lam}", args_s.join(", "));
    }

    // General call.
    let name = camel(method);
    match recv {
        Some(r) => format!("{}.{name}({})", emit_expr(r), args_s.join(", ")),
        None => format!("{name}({})", args_s.join(", ")),
    }
}
