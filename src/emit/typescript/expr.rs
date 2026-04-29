//! Generic TypeScript body / expression / literal emission. Used by
//! the standalone `emit_method` (runtime extraction) and indirectly by
//! controller / view / model / spec emitters that fall back to
//! arbitrary `Expr` rendering.

use super::naming::{ts_field_name, ts_method_name};
use crate::expr::{Expr, ExprNode, LValue, Literal};
use crate::ty::Ty;

// Body + expressions ---------------------------------------------------

pub(super) fn emit_body(body: &Expr, return_ty: &Ty) -> String {
    let is_void = matches!(return_ty, Ty::Nil);
    match &*body.node {
        // `def initialize(owner); @owner = owner; end` — the assignment
        // is the whole body. Emit the assignment as a statement, then
        // return its value if non-void. Without this, the side-effect
        // of setting the ivar is lost (`{};` of the value alone reads
        // the local but doesn't write the ivar).
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            let field = ts_field_name(name.as_str());
            let value_s = emit_expr(value);
            if is_void {
                format!("this.{field} = {value_s};")
            } else {
                format!("return this.{field} = {value_s};")
            }
        }
        ExprNode::Seq { exprs } if !exprs.is_empty() => {
            let mut lines: Vec<String> = Vec::new();
            for (i, e) in exprs.iter().enumerate() {
                lines.push(emit_stmt(e, i == exprs.len() - 1, is_void));
            }
            lines.join("\n")
        }
        // Method-body-level begin/rescue emits as native try/catch rather
        // than IIFE-wrapped. Preserves control flow: early `return` inside
        // the body actually exits the method, `throw e` outside the match
        // arms rethrows cleanly, and no needless `(() => { ... })()` noise.
        ExprNode::BeginRescue { body: inner, rescues, else_branch, ensure, .. } => {
            emit_begin_rescue_stmt(inner, rescues, else_branch.as_ref(), ensure.as_ref(), return_ty)
        }
        _ => {
            if is_void {
                format!("{};", emit_expr(body))
            } else {
                format!("return {};", emit_expr(body))
            }
        }
    }
}

/// Render a begin/rescue at statement position — inside a method body
/// rather than as an expression. Preserves native TS control flow:
/// `try { ... } catch (e) { ... } finally { ... }` with early-return
/// and rethrow working as Ruby's semantics expect.
fn emit_begin_rescue_stmt(
    body: &Expr,
    rescues: &[crate::expr::RescueClause],
    else_branch: Option<&Expr>,
    ensure: Option<&Expr>,
    return_ty: &Ty,
) -> String {
    let mut out = String::new();
    out.push_str("try {\n");
    let body_s = emit_body(body, return_ty);
    for line in body_s.lines() {
        out.push_str("  ");
        out.push_str(line);
        out.push('\n');
    }
    if let Some(eb) = else_branch {
        // Ruby's `else` runs iff the body raised nothing. Appending to
        // the try block preserves that ordering.
        let eb_s = emit_body(eb, return_ty);
        for line in eb_s.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("} catch (e) {\n");

    // Chain rescue clauses as `if (e instanceof X) { ... } else if ...
    // else { throw e; }`. Bare rescue (no classes) is the catchall.
    let mut bare_catchall = false;
    for (i, rc) in rescues.iter().enumerate() {
        let body_s = emit_body(&rc.body, return_ty);
        let indented = indent_block(&body_s, 4);
        if rc.classes.is_empty() {
            out.push_str("  ");
            out.push_str(&indented.trim_start().to_string());
            out.push('\n');
            bare_catchall = true;
            break;
        }
        let instanceof_s: Vec<String> = rc
            .classes
            .iter()
            .map(|c| format!("e instanceof {}", emit_expr(c)))
            .collect();
        let keyword = if i == 0 { "if" } else { "} else if" };
        out.push_str("  ");
        out.push_str(&format!("{keyword} ({}) {{\n", instanceof_s.join(" || ")));
        for line in body_s.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    if !bare_catchall && !rescues.is_empty() {
        out.push_str("  } else {\n");
        out.push_str("    throw e;\n");
        out.push_str("  }\n");
    } else if rescues.is_empty() {
        // `begin; body; ensure; ...; end` with no rescue — must still
        // rethrow in the catch to preserve exception propagation.
        out.push_str("  throw e;\n");
    }
    out.push_str("}");

    if let Some(en) = ensure {
        out.push_str(" finally {\n");
        let en_s = emit_body(en, &Ty::Nil);
        for line in en_s.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("}");
    }
    out
}

fn indent_block(s: &str, spaces: usize) -> String {
    let pad = " ".repeat(spaces);
    s.lines()
        .map(|l| format!("{pad}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_stmt(e: &Expr, is_last: bool, void_return: bool) -> String {
    match &*e.node {
        ExprNode::Assign { target: LValue::Var { name, .. }, value } => {
            format!("const {} = {};", name, emit_expr(value))
        }
        ExprNode::Assign { target: LValue::Ivar { name }, value } => {
            format!("this.{} = {};", ts_field_name(name.as_str()), emit_expr(value))
        }
        // Return at statement position: emit as a native `return`
        // rather than wrapping in an IIFE. Ruby's `return nil` returns
        // nil, not undefined — emit `return null;` (not bare `return;`)
        // to preserve that semantic under TS's strict equality rules.
        ExprNode::Return { value } => {
            format!("return {};", emit_expr(value))
        }
        // Guard-return pattern: `if (cond) { return X; }` at statement
        // position, with no else branch (or an else that's nil). Rather
        // than emit a ternary, produce a native guard — preserves the
        // Ruby idiom `return nil if cond` as idiomatic TS.
        ExprNode::If { cond, then_branch, else_branch }
            if matches!(&*then_branch.node, ExprNode::Return { .. })
                && is_nil_or_empty(else_branch) =>
        {
            if let ExprNode::Return { value } = &*then_branch.node {
                format!("if ({}) return {};", emit_expr(cond), emit_expr(value))
            } else {
                unreachable!()
            }
        }
        // Postfix-`if` at statement position with no else branch.
        // Ruby's `x = [] if x.nil?` lowers to `If { cond, then=Assign,
        // else=nil }`. The default arm below would route through
        // `emit_expr` (a ternary), which drops the assignment's LHS
        // (`Assign` in expression position emits only the rhs). Emit
        // a native `if (cond) <stmt>;` instead — preserves the side
        // effect.
        ExprNode::If { cond, then_branch, else_branch } if is_nil_or_empty(else_branch) => {
            let cond_s = emit_expr(cond);
            let then_stmt = emit_stmt(then_branch, false, true);
            // emit_stmt with void_return=true gives a side-effect-only
            // form (no `return` wrapping). Already includes its own
            // trailing semicolon.
            format!("if ({cond_s}) {then_stmt}")
        }
        _ => {
            if is_last && !void_return {
                format!("return {};", emit_expr(e))
            } else {
                format!("{};", emit_expr(e))
            }
        }
    }
}

fn is_nil_or_empty(e: &Expr) -> bool {
    matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Nil }
            | ExprNode::Seq { .. }  // empty Seq also falls here
    ) && matches!(
        &*e.node,
        ExprNode::Lit { value: Literal::Nil }
            | ExprNode::Seq { exprs: _ }
    ) && {
        if let ExprNode::Seq { exprs } = &*e.node {
            exprs.is_empty()
        } else {
            matches!(&*e.node, ExprNode::Lit { value: Literal::Nil })
        }
    }
}

pub(super) fn emit_expr(e: &Expr) -> String {
    // Analyzer-set diagnostic annotations short-circuit to a target
    // raise-equivalent (preserves Ruby's runtime-raise semantics).
    if e.diagnostic.is_some() {
        return r#"(() => { throw new Error("roundhouse: + with incompatible operand types"); })()"#.to_string();
    }
    match &*e.node {
        ExprNode::Lit { value } => emit_literal(value),
        ExprNode::Const { path } => {
            path.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(".")
        }
        ExprNode::Var { name, .. } => name.to_string(),
        ExprNode::Ivar { name } => format!("this.{}", ts_field_name(name.as_str())),
        ExprNode::Send { recv, method, args, block, parenthesized } => {
            emit_send_with_block(
                recv.as_ref(),
                method.as_str(),
                args,
                block.as_ref(),
                *parenthesized,
            )
        }
        ExprNode::Assign { target: _, value } => emit_expr(value),
        ExprNode::Seq { exprs } => {
            exprs.iter().map(emit_expr).collect::<Vec<_>>().join("; ")
        }
        ExprNode::If { cond, then_branch, else_branch } => {
            // TS ternary `cond ? a : b`. `emit_expr` is always called in
            // an expression position; controller/view emitters have
            // their own statement-form If handlers.
            let cond_s = emit_expr(cond);
            let then_s = emit_expr(then_branch);
            let else_s = emit_expr(else_branch);
            format!("{cond_s} ? {then_s} : {else_s}")
        }
        ExprNode::BoolOp { op, left, right, .. } => {
            use crate::expr::BoolOpKind;
            let op_s = match op {
                BoolOpKind::Or => "||",
                BoolOpKind::And => "&&",
            };
            format!("{} {} {}", emit_expr(left), op_s, emit_expr(right))
        }
        ExprNode::Array { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
        }
        ExprNode::Hash { entries, .. } => {
            let parts: Vec<String> = entries
                .iter()
                .map(|(k, v)| format!("{}: {}", emit_expr(k), emit_expr(v)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        ExprNode::StringInterp { parts } => {
            use crate::expr::InterpPart;
            let mut out = String::from("`");
            for p in parts {
                match p {
                    InterpPart::Text { value } => {
                        for c in value.chars() {
                            if c == '`' || c == '\\' {
                                out.push('\\');
                                out.push(c);
                            } else if c == '$' {
                                out.push_str("\\$");
                            } else {
                                out.push(c);
                            }
                        }
                    }
                    InterpPart::Expr { expr } => {
                        out.push_str("${");
                        out.push_str(&emit_expr(expr));
                        out.push('}');
                    }
                }
            }
            out.push('`');
            out
        }
        ExprNode::SelfRef => "this".to_string(),
        ExprNode::Lambda { params, body, .. } => {
            let params_s: Vec<String> = params.iter().map(|p| p.as_str().to_string()).collect();
            let body_s = emit_expr(body);
            match params.len() {
                0 => format!("() => {body_s}"),
                1 => format!("{} => {body_s}", params_s[0]),
                _ => format!("({}) => {body_s}", params_s.join(", ")),
            }
        }
        ExprNode::Return { value } => {
            // Expression-position return is rare — typically the
            // statement-level emitter handles Return cleanly. An IIFE
            // preserves semantics when Return appears inside a larger
            // expression (e.g., ternary guard `cond ? (return x) : y`).
            format!("(() => {{ return {}; }})()", emit_expr(value))
        }
        ExprNode::Super { args } => {
            // Ruby's `super` forwards to the parent class's same-named
            // method. TS requires `super.methodName(...)`, which needs
            // enclosing-method context that this emitter doesn't carry.
            // Emit syntactically-valid `super(...)` — class-level
            // emitters rewrite to `super.X(...)` where they know X.
            let args_s: Vec<String> = match args {
                None => vec![],
                Some(a) => a.iter().map(emit_expr).collect(),
            };
            format!("super({})", args_s.join(", "))
        }
        ExprNode::BeginRescue { body, rescues, ensure, .. } => {
            // Expression-position begin/rescue — wrap the try/catch in
            // an IIFE so the whole thing evaluates to a value. Single
            // bare `rescue` is common; multi-clause becomes an
            // instanceof chain in the catch body.
            let body_s = emit_expr(body);
            let catch_body = build_catch_body(rescues);
            let ensure_s = match ensure {
                Some(e) => format!(" finally {{ {}; }}", emit_expr(e)),
                None => String::new(),
            };
            format!(
                "(() => {{ try {{ return {body_s}; }} catch (e) {{ {catch_body} }}{ensure_s} }})()"
            )
        }
        ExprNode::RescueModifier { expr, fallback } => {
            format!(
                "(() => {{ try {{ return {}; }} catch {{ return {}; }} }})()",
                emit_expr(expr),
                emit_expr(fallback)
            )
        }
        ExprNode::Yield { args } => {
            // Ruby's `yield` invokes the enclosing method's implicit
            // block. Library-class emit gives every yield-using method
            // an injected `__block` parameter (see emit_plain_method);
            // here we just call it. Yield always targets the enclosing
            // *method*, not surrounding lambdas, so a naive substitution
            // is safe — the caller arranges __block to be in scope.
            let args_s: Vec<String> = args.iter().map(emit_expr).collect();
            format!("__block({})", args_s.join(", "))
        }
        other => format!("/* TODO: emit {:?} */", std::mem::discriminant(other)),
    }
}

fn build_catch_body(rescues: &[crate::expr::RescueClause]) -> String {
    if rescues.is_empty() {
        return "throw e;".to_string();
    }
    // Bare rescue (no explicit classes) catches everything.
    if rescues.len() == 1 && rescues[0].classes.is_empty() {
        return format!("return {};", emit_expr(&rescues[0].body));
    }
    let mut out = String::new();
    let mut has_bare_catchall = false;
    for (i, rc) in rescues.iter().enumerate() {
        if rc.classes.is_empty() {
            out.push_str(&format!(" else {{ return {}; }}", emit_expr(&rc.body)));
            has_bare_catchall = true;
            break;
        }
        let keyword = if i == 0 { "if" } else { "else if" };
        let instanceof_s: Vec<String> = rc
            .classes
            .iter()
            .map(|c| format!("e instanceof {}", emit_expr(c)))
            .collect();
        out.push_str(&format!(
            "{keyword} ({}) {{ return {}; }}",
            instanceof_s.join(" || "),
            emit_expr(&rc.body)
        ));
    }
    if !has_bare_catchall {
        out.push_str(" else { throw e; }");
    }
    out
}

/// Core send emission. `parenthesized` reflects whether the Ruby
/// source wrapped args in explicit parens — for 0-arg explicit-
/// receiver calls we use it to decide between `recv.name` (Ruby
/// reader convention, JS property access) and `recv.name()` (method
/// call). Always emits parens when args are present.
/// Send emission that folds a trailing block (Ruby `do ... end` / `&:sym`)
/// in as an arrow-function argument — TS's closest equivalent. The
/// block-less path delegates directly to `emit_send_with_parens`.
pub(super) fn emit_send_with_block(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    block: Option<&Expr>,
    parenthesized: bool,
) -> String {
    let Some(blk) = block else {
        return emit_send_with_parens(recv, method, args, parenthesized);
    };
    let mut all_args: Vec<Expr> = args.to_vec();
    all_args.push(blk.clone());
    emit_send_with_parens(recv, method, &all_args, true)
}

pub(super) fn emit_send_with_parens(
    recv: Option<&Expr>,
    method: &str,
    args: &[Expr],
    parenthesized: bool,
) -> String {
    let args_s: Vec<String> = args.iter().map(emit_expr).collect();
    if method == "[]" && recv.is_some() {
        return format!("{}[{}]", emit_expr(recv.unwrap()), args_s.join(", "));
    }
    // `recv.[]=(k, v)` — indexed assignment lowered to a Send. TS needs
    // the LHS form `recv[k] = v`.
    if method == "[]=" && recv.is_some() && args.len() == 2 {
        return format!(
            "{}[{}] = {}",
            emit_expr(recv.unwrap()),
            args_s[0],
            args_s[1]
        );
    }
    // `Target.new(args)` → `new Target(args)`. Ruby's standard constructor
    // call convention; Juntos-side classes use the JS `new` keyword.
    if method == "new" && recv.is_some() {
        let recv_s = emit_expr(recv.unwrap());
        if args_s.is_empty() {
            return format!("new {recv_s}()");
        }
        return format!("new {recv_s}({})", args_s.join(", "));
    }
    // `x.nil?` → `x === null`. Ruby's `nil?` only matches nil (not
    // `false`, and TS equivalent must distinguish from `undefined`).
    // Strict equality preserves semantics.
    if method == "nil?" && recv.is_some() && args.is_empty() {
        return format!("{} === null", emit_expr(recv.unwrap()));
    }
    // `x.!` — the Send-channel form of unary `!` (e.g., `!cond` lowered
    // to `cond.!`). Emit TS's prefix `!`.
    if method == "!" && recv.is_some() && args.is_empty() {
        return format!("!{}", emit_expr(recv.unwrap()));
    }
    // Type-aware Ruby Enumerable → JS Array method rename. JS arrays
    // don't have `.each` (they have `.forEach`) and use `.length` not
    // `.size`. When the analyzer has typed the receiver as Array, emit
    // the JS-native form.
    if let Some(r) = recv {
        if let Some(Ty::Array { .. }) = &r.ty {
            match method {
                "each" => {
                    let recv_s = emit_expr(r);
                    return if args_s.is_empty() {
                        format!("{recv_s}.forEach")
                    } else {
                        format!("{recv_s}.forEach({})", args_s.join(", "))
                    };
                }
                "size" if args.is_empty() => {
                    return format!("{}.length", emit_expr(r));
                }
                _ => {}
            }
        }
    }

    // Ruby's `<<` is polymorphic: Int bit-shift, Array/String append,
    // or a method call on classes that define it (like
    // ActiveModel::Errors.add). Dispatch on receiver type. TS has no
    // `<<` operator overloading, so the Class case has to emit as a
    // method call; the method name is `add` by convention (matches
    // Juntos's ActiveModel::Errors and similar collection APIs).
    if method == "<<" && recv.is_some() && args.len() == 1 {
        let r = recv.unwrap();
        if let Some(recv_ty) = &r.ty {
            match recv_ty {
                Ty::Class { .. } => {
                    return format!("{}.add({})", emit_expr(r), args_s[0]);
                }
                Ty::Array { .. } => {
                    return format!("{}.push({})", emit_expr(r), args_s[0]);
                }
                _ => {}
            }
        }
    }
    // Ruby's binary operators ride the Send channel. TS needs infix;
    // `==` and `!=` map to strict `===` / `!==` so equality semantics
    // match Ruby (Ruby has no implicit type coercion).
    if let (Some(r), [arg]) = (recv, args) {
        // `+` dispatch: TS's native `+` handles numeric and string;
        // Array concat wants spread. Incompatible pairs refuse.
        if method == "+" {
            use crate::emit::shared::add::{classify_add, AddCase};
            match classify_add(r, arg) {
                AddCase::ArrayConcat { .. } => {
                    return format!("[...{}, ...{}]", emit_expr(r), emit_expr(arg));
                }
                AddCase::Incompatible => {
                    // Emit a runtime throw via IIFE; `throw` is a
                    // statement in JS/TS so wrapping is required to
                    // keep the form expression-valued.
                    return r#"(() => { throw new Error("roundhouse: + with incompatible operand types"); })()"#.to_string();
                }
                _ => {}
            }
        }
        // `-` dispatch: TS's native `-` handles numerics. Array set-
        // difference uses filter + includes. Incompatible pairs refuse.
        if method == "-" {
            use crate::emit::shared::sub::{classify_sub, SubCase};
            match classify_sub(r, arg) {
                SubCase::ArrayDifference { .. } => {
                    return format!(
                        "{}.filter(x => !{}.includes(x))",
                        emit_expr(r),
                        emit_expr(arg)
                    );
                }
                SubCase::Incompatible => {
                    return r#"(() => { throw new Error("roundhouse: - with incompatible operand types"); })()"#.to_string();
                }
                _ => {}
            }
        }
        // `*` dispatch: TS's native `*` handles numerics. String repeat
        // uses `.repeat(n)`; array repeat has no built-in (flat
        // map-ish trick); array join uses `.join(sep)`.
        if method == "*" {
            use crate::emit::shared::mul::{classify_mul, MulCase};
            match classify_mul(r, arg) {
                MulCase::StringRepeat => {
                    return format!("{}.repeat({})", emit_expr(r), emit_expr(arg));
                }
                MulCase::ArrayRepeat { .. } => {
                    // Array(n).fill(lhs).flat() repeats the array n times.
                    return format!(
                        "Array({}).fill({}).flat()",
                        emit_expr(arg),
                        emit_expr(r)
                    );
                }
                MulCase::ArrayJoin { .. } => {
                    return format!("{}.join({})", emit_expr(r), emit_expr(arg));
                }
                MulCase::Incompatible => {
                    return r#"(() => { throw new Error("roundhouse: * with incompatible operand types"); })()"#.to_string();
                }
                _ => {}
            }
        }
        // `/` and `**` dispatch: TS has both as native operators. Only
        // Incompatible pairs need special handling.
        if method == "/" || method == "**" {
            use crate::emit::shared::div_pow::{classify_div_pow, DivPowCase};
            if matches!(classify_div_pow(r, arg), DivPowCase::Incompatible) {
                return format!(
                    r#"(() => {{ throw new Error("roundhouse: `{method}` with incompatible operand types"); }})()"#
                );
            }
        }
        // `%` dispatch: TS has native `%` for numerics; Str % args
        // (Ruby sprintf) has no JS/TS equivalent — emit a throw.
        if method == "%" {
            use crate::emit::shared::modulo::{classify_modulo, ModuloCase};
            match classify_modulo(r, arg) {
                ModuloCase::StringFormat => {
                    return r#"(() => { throw new Error("roundhouse: String % (sprintf) not yet supported for TypeScript target"); })()"#.to_string();
                }
                ModuloCase::Incompatible => {
                    return r#"(() => { throw new Error("roundhouse: % with incompatible operand types"); })()"#.to_string();
                }
                _ => {}
            }
        }
        if let Some(op) = ts_binop(method) {
            return format!("{} {op} {}", emit_expr(r), emit_expr(arg));
        }
    }
    // Ruby stdlib method → TS equivalent, when the Ruby name collides
    // with a nonexistent TS property. Keyed on name only today; a
    // receiver-typed dispatch would replace this when per-type
    // mappings diverge.
    let (mapped_name, force_parens) = match method {
        "strip" => ("trim", true),
        _ => (method, false),
    };
    let ts_m = ts_method_name(mapped_name);
    match recv {
        None => {
            if args_s.is_empty() {
                ts_m
            } else {
                format!("{}({})", ts_m, args_s.join(", "))
            }
        }
        Some(r) => {
            let recv_s = emit_expr(r);
            if args_s.is_empty() && !parenthesized && !force_parens {
                // Ruby's `obj.name` without parens is typically a
                // reader; Juntos mirrors that with a property
                // accessor / getter, so emit without parens.
                format!("{recv_s}.{ts_m}")
            } else {
                format!("{recv_s}.{ts_m}({})", args_s.join(", "))
            }
        }
    }
}

pub(super) fn emit_literal(lit: &Literal) -> String {
    match lit {
        Literal::Nil => "null".to_string(),
        Literal::Bool { value } => value.to_string(),
        Literal::Int { value } => value.to_string(),
        Literal::Float { value } => {
            let s = value.to_string();
            if s.contains('.') { s } else { format!("{s}.0") }
        }
        Literal::Str { value } => format!("{value:?}"),
        // Ruby symbols map to string literals — the typed analyzer may
        // refine this into a discriminated-union enum later, but for
        // the scaffold a string is unambiguous and round-trips through
        // comparison as expected.
        Literal::Sym { value } => format!("{:?}", value.as_str()),
        Literal::Regex { pattern, flags } => format!("/{pattern}/{flags}"),
    }
}

fn ts_binop(method: &str) -> Option<&'static str> {
    Some(match method {
        "==" => "===",
        "!=" => "!==",
        "<" => "<",
        "<=" => "<=",
        ">" => ">",
        ">=" => ">=",
        "+" => "+",
        "-" => "-",
        "*" => "*",
        "/" => "/",
        "%" => "%",
        "**" => "**",
        "<<" => "<<",
        ">>" => ">>",
        "|" => "|",
        "&" => "&",
        "^" => "^",
        _ => return None,
    })
}
