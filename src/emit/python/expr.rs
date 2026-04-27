//! Generic Python expression / body / statement renderer.
//!
//! Reused by the model-method emitter and as a fallback for the
//! controller-action walker.

use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

// Bodies + expressions -------------------------------------------------

pub(super) fn emit_body(body: &Expr, return_ty: &Ty) -> String {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        ExprNode::Assign { target: LValue::Ivar { .. }, value } => {
            if is_void {
                emit_expr(value)
            } else {
                format!("return {}", emit_expr(value))
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                if i > 0 && e.leading_blank_line {
                    lines.push(String::new());
                }
                lines.push(emit_stmt(e, i == exprs.len() - 1, is_void));
            }
            lines.join("\n")
        }
        _ => {
            if is_void {
                emit_expr(body)
            } else {
                format!("return {}", emit_expr(body))
            }
        }
    }
}

pub(super) fn emit_stmt(e: &Expr, is_last: bool, void_return: bool) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("{} = {}", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("self.{} = {}", name, emit_expr(value))
        }
        _ => {
            if is_last && !void_return {
                format!("return {}", emit_expr(e))
            } else {
                emit_expr(e)
            }
        }
    }
}

pub(super) fn emit_expr(e: &Expr) -> String {
    // Analyze may have annotated this expression as a user error
    // (e.g., Incompatible `+`). If so, emit the target raise-
    // equivalent instead of the normal rendering — matches Ruby's
    // behavior of raising at runtime.
    if e.diagnostic.is_some() {
        return r#"(_ for _ in ()).throw(TypeError("roundhouse: + with incompatible operand types"))"#.to_string();
    }
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("self.{name}"),
        ExprNode::Send { recv, method, args, .. } => {
            emit_send(recv.as_ref(), method.as_str(), args)
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // Python ternary `a if cond else b`. `emit_expr` is always
            // called in an expression position (non-expression If flow
            // lives in the controller/view emitters with their own
            // statement-form handlers).
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_s = emit_expr(else_branch);
            format!("{then_s} if {cond_s} else {else_s}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "or",
                BoolOpKind::And => "and",
            };
            format!("{} {op_s} {}", emit_expr(left), emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    // Symbol keys become string keys in Python dicts
                    // (since Python has no atom type). Rails-style
                    // `{foo: 1}` → `{"foo": 1}`.
                    if let ExprNode::Lit { value: Literal::Sym { value } } = &*k.node {
                        format!("{:?}: {}", value.as_str(), emit_expr(v))
                    } else {
                        format!("{}: {}", emit_expr(k), emit_expr(v))
                    }
                })
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            // Python f-string — `f"text {expr} more"`. Reserved chars
            // inside the `{}` are rare for the Ruby expressions we
            // ingest, so no escape bookkeeping yet; if a fixture
            // triggers one we add it then.
            use crate::expr::InterpPart;
            let mut out = String::from("f\"");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            match c {
                                '"' => out.push_str("\\\""),
                                '\\' => out.push_str("\\\\"),
                                '{' => out.push_str("{{"),
                                '}' => out.push_str("}}"),
                                '\n' => out.push_str("\\n"),
                                other => out.push(other),
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push('{');
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('"');
            out
        }
        other => format!("# TODO: emit {:?}", std::mem::discriminant(other)),
    }
}

pub(super) fn emit_send(recv: Option<&Expr>, method: &str, args: &[Expr]) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }
    // Ruby's binary operators ride the Send channel (`a == b` is
    // `a.==(b)`). Python needs infix; emit as `recv op arg` for the
    // ones whose syntax matches 1:1. Equality against Nil gets the
    // idiomatic `is None` / `is not None` form when the body-typer
    // flagged one side as Ty::Nil.
    if let (Some(r), [arg]) = (recv, args) {
        if method == "==" || method == "!=" {
            use crate::emit::shared::eq::{classify_eq, EqCase};
            if let EqCase::NilCheck { subject } = classify_eq(r, arg) {
                let keyword = if method == "==" { "is" } else { "is not" };
                return format!("{} {keyword} None", emit_expr(subject));
            }
        }
        // `+` dispatch: Python's native `+` handles all of the
        // supported cases (numeric, string, list). The dispatch's
        // only behavior change is rejecting Incompatible pairs
        // (Int+Str, Hash+Hash, …) that Ruby would raise on.
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            if matches!(classify_add(r, arg), AddCase::Incompatible) {
                // Emit a runtime raise in Python, matching Ruby's
                // behavior (Ruby would raise TypeError at this line).
                // `raise` is a statement, so use a generator `.throw`
                // trick to keep the form expression-valued.
                return r#"(_ for _ in ()).throw(TypeError("roundhouse: + with incompatible operand types"))"#.to_string();
            }
        }
        // `-` dispatch: Python supports numeric `-` natively; list
        // difference needs a comprehension. Incompatible refuses.
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            match classify_sub(r, arg) {
                SubCase::ArrayDifference { .. } => {
                    return format!(
                        "[x for x in {} if x not in {}]",
                        emit_expr(r),
                        emit_expr(arg)
                    );
                }
                SubCase::Incompatible => {
                    return r#"(_ for _ in ()).throw(TypeError("roundhouse: - with incompatible operand types"))"#.to_string();
                }
                _ => {}
            }
        }
        // `*` dispatch: Python's native `*` handles numeric, string
        // repeat, and list repeat. Only array-join needs `.join(...)`;
        // Incompatible pairs refuse.
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            match classify_mul(r, arg) {
                MulCase::ArrayJoin { .. } => {
                    // `sep.join(str(x) for x in arr)` — Python's idiom.
                    return format!(
                        "{}.join(str(x) for x in {})",
                        emit_expr(arg),
                        emit_expr(r)
                    );
                }
                MulCase::Incompatible => {
                    return r#"(_ for _ in ()).throw(TypeError("roundhouse: * with incompatible operand types"))"#.to_string();
                }
                _ => {}
            }
        }
        // `/` and `**` dispatch: Python has both natively. Ruby's Int/Int
        // is integer division (towards -infinity); Python's `/` is true
        // division, and `//` is floor. For now emit `/` unconditionally;
        // refine if an Int/Int case forces the floor-div distinction.
        if method == "/" || method == "**" {
            use crate::emit::shared::div_pow::{classify_div_pow, DivPowCase};
            if matches!(classify_div_pow(r, arg), DivPowCase::Incompatible) {
                return format!(
                    r#"(_ for _ in ()).throw(TypeError("roundhouse: `{method}` with incompatible operand types"))"#
                );
            }
        }
        // `%` dispatch: Python's native `%` covers numeric and string
        // format directly (same printf-style as Ruby). Only refuse
        // Incompatible pairs.
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            if matches!(classify_modulo(r, arg), ModuloCase::Incompatible) {
                return r#"(_ for _ in ()).throw(TypeError("roundhouse: % with incompatible operand types"))"#.to_string();
            }
        }
        if is_py_binop(method) {
            return format!("{} {} {}", emit_expr(r), method, emit_expr(arg));
        }
    }
    match recv {
        None => {
            if args_s.is_empty() {
                method.to_string()
            } else {
                format!("{}({})", method, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() {
                // Bare `recv.method` (no parens) is how Python accesses
                // attributes. For method calls with no args we emit
                // `recv.method()` — matches Python idiom and avoids
                // confusing a 0-arity call with an attribute read.
                format!("{recv_s}.{method}()")
            } else {
                format!("{recv_s}.{method}({})", args_s.join(", "))
            }
        }
    }
}

fn is_py_binop(method: &str) -> bool {
    matches!(
        method,
        "==" | "!="
            | "<"
            | "<="
            | ">"
            | ">="
            | "+"
            | "-"
            | "*"
            | "/"
            | "%"
            | "**"
            | "<<"
            | ">>"
            | "|"
            | "&"
            | "^"
    )
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "None".to_string(),
        Literal::Bool { value } => {
            if *value { "True".to_string() } else { "False".to_string() }
        }
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        // Symbols have no direct Python equivalent; emit as string
        // literals. Enum detection would refine this into a typed
        // Enum subclass later.
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, flags } => {
            let flag_expr = python_regex_flag_expr(flags);
            if flag_expr.is_empty() {
                format!("re.compile({pattern:?})")
            } else {
                format!("re.compile({pattern:?}, {flag_expr})")
            }
        }
    }
}

fn python_regex_flag_expr(flags: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if flags.contains('i') { parts.push("re.IGNORECASE"); }
    if flags.contains('m') { parts.push("re.MULTILINE"); }
    if flags.contains('x') { parts.push("re.VERBOSE"); }
    parts.join(" | ")
}
